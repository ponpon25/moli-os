//! A camera as an ONVIF device (Profile S, the part a recorder needs to
//! adopt it): UniFi Protect adopts only ONVIF cameras.
//!
//! One HTTP port per camera (a recorder takes one device per address and
//! port): `/onvif/device_service`, `/onvif/media_service` (SOAP 1.2), and
//! `/onvif/snapshot.jpg`. The stream itself stays the RTSP surface's, under
//! the same user and password.
//!
//! What is answered is what recorders ask to adopt a camera and find its
//! stream: the device's information, time, capabilities, services and
//! scopes, its profiles (a main and a sub one, both the camera's one stream:
//! Protect expects two), video sources and encoder configurations, the
//! stream and snapshot addresses. Anything else is a SOAP fault
//! `ActionNotSupported`.
//!
//! Credentials:
//! - a request that carries some (WS-Security `UsernameToken`, digest or
//!   text, or HTTP Digest) must carry the right ones, or it is refused
//!   (`NotAuthorized`): the recorder learns at adoption that the password
//!   is wrong;
//! - without any, the descriptions are given (they hold no secret: the
//!   stream they point to asks for the password), the image is not;
//! - the image asks for HTTP Digest.

use std::fmt::Write as _;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use moli_core::DeviceId;
use ring::digest::{SHA1_FOR_LEGACY_USE_ONLY, digest};

use crate::connection::Shared;
use crate::digest as http_digest;

/// Largest SOAP request read.
const MAX_BODY: usize = 64 * 1024;
/// A WS-Security `Created` further than this from Moli's clock is refused.
const CLOCK_SKEW: i64 = 15 * 60;

/// One camera served as an ONVIF device.
#[derive(Debug)]
pub struct Device {
    pub path: String,
    pub name: String,
    pub id: DeviceId,
    pub port: u16,
    pub shared: Arc<Shared>,
}

impl Device {
    /// The device's stable identity, from its Moli id: a UUID for
    /// discovery, a serial number for the recorder.
    pub fn uuid(&self) -> String {
        uuid_of(self.id.as_str())
    }
}

/// A UUID (version 5 shape) from a name: stable across restarts.
#[must_use]
pub fn uuid_of(name: &str) -> String {
    let h = digest(
        &ring::digest::SHA256,
        format!("moli-onvif:{name}").as_bytes(),
    );
    let mut b = [0u8; 16];
    b.copy_from_slice(&h.as_ref()[..16]);
    b[6] = (b[6] & 0x0F) | 0x50;
    b[8] = (b[8] & 0x3F) | 0x80;
    let hex: String = b.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    });
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

// ---- a little XML ------------------------------------------------------------

/// Text made safe inside an element or an attribute.
#[must_use]
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// The tags of a document, in order: (local name, is closing, byte range of
/// the whole tag).
fn tags(xml: &str) -> impl Iterator<Item = (&str, bool, usize, usize)> {
    xml.match_indices('<').filter_map(move |(start, _)| {
        let rest = &xml[start + 1..];
        if rest.starts_with(['?', '!']) {
            return None;
        }
        let end = start + 1 + rest.find('>')?;
        let inner = &xml[start + 1..end];
        let closing = inner.starts_with('/');
        let name = inner
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()?;
        let local = name.rsplit(':').next()?;
        Some((local, closing, start, end))
    })
}

/// The text of the first element named `local` (any prefix).
#[must_use]
pub fn element<'a>(xml: &'a str, local: &str) -> Option<&'a str> {
    let (_, _, _, end) = tags(xml).find(|(name, closing, _, _)| *name == local && !closing)?;
    if xml[..end].ends_with('/') {
        return Some("");
    }
    let content = &xml[end + 1..];
    let stop = content.find("</")?;
    Some(content[..stop].trim())
}

/// The local name of the first element inside the SOAP `Body`: the action.
#[must_use]
pub fn action(xml: &str) -> Option<&str> {
    let mut seen_body = false;
    for (name, closing, _, _) in tags(xml) {
        if closing {
            continue;
        }
        if seen_body {
            return Some(name);
        }
        seen_body = name == "Body";
    }
    None
}

fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0;
    for c in text.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            #[allow(clippy::cast_possible_truncation)] // one byte
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

// ---- credentials -------------------------------------------------------------

/// What a request's credentials prove.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Credentials {
    /// None given.
    Absent,
    Proved,
    Wrong,
}

/// WS-Security `UsernameToken` (ONVIF Core §5.12): digest
/// `base64(sha1(nonce + created + password))`, or the password in text.
#[must_use]
pub fn ws_security(xml: &str, user: &str, password: Option<&str>, now: i64) -> Credentials {
    let Some(token) = element_block(xml, "UsernameToken") else {
        return Credentials::Absent;
    };
    let Some(password) = password else {
        return Credentials::Wrong;
    };
    let (Some(username), Some(given)) = (element(token, "Username"), element(token, "Password"))
    else {
        return Credentials::Wrong;
    };
    if username != user {
        return Credentials::Wrong;
    }
    let is_text = tags(token)
        .find(|(n, c, _, _)| *n == "Password" && !c)
        .is_some_and(|(_, _, s, e)| token[s..e].contains("#PasswordText"));
    if is_text {
        return if equal(given, password) {
            Credentials::Proved
        } else {
            Credentials::Wrong
        };
    }
    let (Some(nonce), Some(created)) = (element(token, "Nonce"), element(token, "Created")) else {
        return Credentials::Wrong;
    };
    // Old tokens are not replayed for long.
    let Ok(at) = created.parse::<jiff::Timestamp>() else {
        return Credentials::Wrong;
    };
    if (at.as_second() - now).abs() > CLOCK_SKEW {
        tracing::warn!(created, "onvif: a token far from Moli's clock is refused");
        return Credentials::Wrong;
    }
    let Some(nonce) = base64_decode(nonce).filter(|n| n.len() <= 64) else {
        return Credentials::Wrong;
    };
    let mut input = nonce;
    input.extend_from_slice(created.as_bytes());
    input.extend_from_slice(password.as_bytes());
    let expected = crate::connection::base64(digest(&SHA1_FOR_LEGACY_USE_ONLY, &input).as_ref());
    if equal(given, &expected) {
        Credentials::Proved
    } else {
        Credentials::Wrong
    }
}

/// The inside of the first element named `local` (its children included).
fn element_block<'a>(xml: &'a str, local: &str) -> Option<&'a str> {
    let mut open = None;
    for (name, closing, start, end) in tags(xml) {
        if name != local {
            continue;
        }
        match (open, closing) {
            (None, false) => open = Some(end + 1),
            (Some(from), true) => return xml.get(from..start),
            _ => {}
        }
    }
    None
}

fn equal(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

/// HTTP Digest on a request, if it carries some.
fn http_credentials(headers: &HeaderMap, method: &str, shared: &Shared) -> (Credentials, String) {
    let (nonce, accepted) = shared.nonces.lock().expect("nonces lock").now();
    let Some(h) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    else {
        return (Credentials::Absent, nonce);
    };
    let Some(password) = shared.password() else {
        return (Credentials::Wrong, nonce);
    };
    match http_digest::check(h, method, &accepted, &shared.user, &password) {
        http_digest::Check::Proved => (Credentials::Proved, nonce),
        // An answer to an old nonce: asked again, as if none was given.
        http_digest::Check::Stale => (Credentials::Absent, nonce),
        http_digest::Check::Wrong => (Credentials::Wrong, nonce),
    }
}

// ---- answers -----------------------------------------------------------------

const ENVELOPE_START: &str = concat!(
    r#"<?xml version="1.0" encoding="UTF-8"?>"#,
    r#"<s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope""#,
    r#" xmlns:tds="http://www.onvif.org/ver10/device/wsdl""#,
    r#" xmlns:trt="http://www.onvif.org/ver10/media/wsdl""#,
    r#" xmlns:tt="http://www.onvif.org/ver10/schema""#,
    r#" xmlns:ter="http://www.onvif.org/ver10/error"><s:Body>"#
);
const ENVELOPE_END: &str = "</s:Body></s:Envelope>";

fn soap(status: StatusCode, body: &str) -> Response {
    let mut r = (status, format!("{ENVELOPE_START}{body}{ENVELOPE_END}")).into_response();
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/soap+xml; charset=utf-8"),
    );
    r
}

fn fault(status: StatusCode, code: &str, subcode: &str, reason: &str) -> Response {
    soap(
        status,
        &format!(
            r#"<s:Fault><s:Code><s:Value>s:{code}</s:Value><s:Subcode><s:Value>ter:{subcode}</s:Value></s:Subcode></s:Code><s:Reason><s:Text xml:lang="en">{reason}</s:Text></s:Reason></s:Fault>"#
        ),
    )
}

fn not_authorized() -> Response {
    fault(
        StatusCode::BAD_REQUEST,
        "Sender",
        "NotAuthorized",
        "Sender not authorized",
    )
}

/// The host the client reached Moli by (the address to give back), if it
/// is a plain host or address.
fn host_of(headers: &HeaderMap) -> Option<String> {
    let host = headers.get(header::HOST)?.to_str().ok()?;
    if host.is_empty()
        || host.len() > 255
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':' | b'[' | b']'))
    {
        return None;
    }
    // Without the port: `[::1]:80` → `[::1]`, `a.b:80` → `a.b`.
    let name = if host.starts_with('[') {
        host.split(']').next().map(|h| format!("{h}]"))?
    } else {
        host.split(':').next()?.to_owned()
    };
    Some(name)
}

/// What the stream is like, as last seen (or what the HomeKit doorbell gives).
fn picture(camera: &Device) -> (u32, u32, &'static str) {
    let seen = camera
        .shared
        .seen
        .lock()
        .expect("seen lock")
        .get(&camera.id)
        .copied();
    let (w, h, profile) = seen.unwrap_or((1280, 720, 77));
    let profile = match profile {
        66 => "Baseline",
        77 => "Main",
        88 => "Extended",
        _ => "High",
    };
    (w, h, profile)
}

fn profile(camera: &Device, token: &str) -> String {
    let (w, h, h264) = picture(camera);
    let name = escape(&camera.name);
    format!(
        concat!(
            r#"<tt:Name>{name} {token}</tt:Name>"#,
            r#"<tt:VideoSourceConfiguration token="video_source_config"><tt:Name>video source</tt:Name><tt:UseCount>2</tt:UseCount>"#,
            r#"<tt:SourceToken>video_source</tt:SourceToken><tt:Bounds x="0" y="0" width="{w}" height="{h}"/></tt:VideoSourceConfiguration>"#,
            "{encoder}"
        ),
        name = name,
        token = token,
        w = w,
        h = h,
        encoder = encoder(camera, token, w, h, h264),
    )
}

fn encoder(_camera: &Device, token: &str, w: u32, h: u32, h264: &str) -> String {
    format!(
        concat!(
            r#"<tt:VideoEncoderConfiguration token="encoder_{token}"><tt:Name>encoder {token}</tt:Name><tt:UseCount>1</tt:UseCount>"#,
            r#"<tt:Encoding>H264</tt:Encoding><tt:Resolution><tt:Width>{w}</tt:Width><tt:Height>{h}</tt:Height></tt:Resolution>"#,
            r#"<tt:Quality>4</tt:Quality><tt:RateControl><tt:FrameRateLimit>30</tt:FrameRateLimit><tt:EncodingInterval>1</tt:EncodingInterval>"#,
            r#"<tt:BitrateLimit>2048</tt:BitrateLimit></tt:RateControl><tt:H264><tt:GovLength>30</tt:GovLength><tt:H264Profile>{h264}</tt:H264Profile></tt:H264>"#,
            r#"<tt:Multicast><tt:Address><tt:Type>IPv4</tt:Type><tt:IPv4Address>0.0.0.0</tt:IPv4Address></tt:Address><tt:Port>0</tt:Port><tt:TTL>0</tt:TTL><tt:AutoStart>false</tt:AutoStart></tt:Multicast>"#,
            r#"<tt:SessionTimeout>PT60S</tt:SessionTimeout></tt:VideoEncoderConfiguration>"#
        ),
        token = token,
        w = w,
        h = h,
        h264 = h264,
    )
}

const PROFILES: [&str; 2] = ["main", "sub"];

fn uri(uri: &str) -> String {
    format!(
        "<trt:MediaUri><tt:Uri>{}</tt:Uri><tt:InvalidAfterConnect>false</tt:InvalidAfterConnect><tt:InvalidAfterReboot>false</tt:InvalidAfterReboot><tt:Timeout>PT0S</tt:Timeout></trt:MediaUri>",
        escape(uri)
    )
}

fn date_time() -> String {
    let now = jiff::Zoned::now();
    let utc = now.with_time_zone(jiff::tz::TimeZone::UTC);
    let part = |z: &jiff::Zoned| {
        format!(
            "<tt:Time><tt:Hour>{}</tt:Hour><tt:Minute>{}</tt:Minute><tt:Second>{}</tt:Second></tt:Time><tt:Date><tt:Year>{}</tt:Year><tt:Month>{}</tt:Month><tt:Day>{}</tt:Day></tt:Date>",
            z.hour(),
            z.minute(),
            z.second(),
            z.year(),
            z.month(),
            z.day()
        )
    };
    format!(
        "<tds:GetSystemDateAndTimeResponse><tds:SystemDateAndTime><tt:DateTimeType>NTP</tt:DateTimeType><tt:DaylightSavings>false</tt:DaylightSavings><tt:TimeZone><tt:TZ>UTC0</tt:TZ></tt:TimeZone><tt:UTCDateTime>{}</tt:UTCDateTime><tt:LocalDateTime>{}</tt:LocalDateTime></tds:SystemDateAndTime></tds:GetSystemDateAndTimeResponse>",
        part(&utc),
        part(&utc)
    )
}

/// The answer to one SOAP action.
#[allow(clippy::too_many_lines)] // one arm per action
fn answer(camera: &Device, action: &str, xml: &str, host: &str) -> Response {
    let base = format!("http://{host}:{}", camera.port);
    let device_xaddr = format!("{base}/onvif/device_service");
    let media_xaddr = format!("{base}/onvif/media_service");
    let rtsp = format!("rtsp://{host}:{}/{}", camera.shared.port, camera.path);
    let ok = |body: String| soap(StatusCode::OK, &body);
    match action {
        "GetSystemDateAndTime" => ok(date_time()),
        "GetDeviceInformation" => {
            let serial = camera.uuid().replace('-', "");
            ok(format!(
                "<tds:GetDeviceInformationResponse><tds:Manufacturer>Moli</tds:Manufacturer><tds:Model>{}</tds:Model><tds:FirmwareVersion>{}</tds:FirmwareVersion><tds:SerialNumber>{}</tds:SerialNumber><tds:HardwareId>moli-rtsp</tds:HardwareId></tds:GetDeviceInformationResponse>",
                escape(&camera.name),
                env!("CARGO_PKG_VERSION"),
                &serial[..16]
            ))
        }
        "GetCapabilities" => {
            let category = element(xml, "Category").unwrap_or("All");
            let mut caps = String::new();
            if matches!(category, "All" | "Device" | "") {
                let _ = write!(
                    caps,
                    "<tt:Device><tt:XAddr>{}</tt:XAddr><tt:Network><tt:IPFilter>false</tt:IPFilter><tt:ZeroConfiguration>false</tt:ZeroConfiguration><tt:IPVersion6>false</tt:IPVersion6><tt:DynDNS>false</tt:DynDNS></tt:Network><tt:System><tt:DiscoveryResolve>false</tt:DiscoveryResolve><tt:DiscoveryBye>false</tt:DiscoveryBye><tt:RemoteDiscovery>false</tt:RemoteDiscovery><tt:SystemBackup>false</tt:SystemBackup><tt:SystemLogging>false</tt:SystemLogging><tt:FirmwareUpgrade>false</tt:FirmwareUpgrade><tt:SupportedVersions><tt:Major>2</tt:Major><tt:Minor>5</tt:Minor></tt:SupportedVersions></tt:System><tt:Security><tt:TLS1.1>false</tt:TLS1.1><tt:TLS1.2>false</tt:TLS1.2><tt:OnboardKeyGeneration>false</tt:OnboardKeyGeneration><tt:AccessPolicyConfig>false</tt:AccessPolicyConfig><tt:X.509Token>false</tt:X.509Token><tt:SAMLToken>false</tt:SAMLToken><tt:KerberosToken>false</tt:KerberosToken><tt:RELToken>false</tt:RELToken></tt:Security></tt:Device>",
                    escape(&device_xaddr)
                );
            }
            if matches!(category, "All" | "Media" | "") {
                let _ = write!(
                    caps,
                    "<tt:Media><tt:XAddr>{}</tt:XAddr><tt:StreamingCapabilities><tt:RTPMulticast>false</tt:RTPMulticast><tt:RTP_TCP>true</tt:RTP_TCP><tt:RTP_RTSP_TCP>true</tt:RTP_RTSP_TCP></tt:StreamingCapabilities><tt:Extension><tt:ProfileCapabilities><tt:MaximumNumberOfProfiles>{}</tt:MaximumNumberOfProfiles></tt:ProfileCapabilities></tt:Extension></tt:Media>",
                    escape(&media_xaddr),
                    PROFILES.len()
                );
            }
            if caps.is_empty() {
                return fault(StatusCode::BAD_REQUEST, "Receiver", "NoSuchService", "No such service");
            }
            ok(format!(
                "<tds:GetCapabilitiesResponse><tds:Capabilities>{caps}</tds:Capabilities></tds:GetCapabilitiesResponse>"
            ))
        }
        "GetServices" => ok(format!(
            "<tds:GetServicesResponse><tds:Service><tds:Namespace>http://www.onvif.org/ver10/device/wsdl</tds:Namespace><tds:XAddr>{}</tds:XAddr><tds:Version><tt:Major>2</tt:Major><tt:Minor>5</tt:Minor></tds:Version></tds:Service><tds:Service><tds:Namespace>http://www.onvif.org/ver10/media/wsdl</tds:Namespace><tds:XAddr>{}</tds:XAddr><tds:Version><tt:Major>2</tt:Major><tt:Minor>5</tt:Minor></tds:Version></tds:Service></tds:GetServicesResponse>",
            escape(&device_xaddr),
            escape(&media_xaddr)
        )),
        "GetScopes" => {
            let scopes: String = scopes(camera)
                .iter()
                .fold(String::new(), |mut s, scope| {
                    let _ = write!(
                        s,
                        "<tds:Scopes><tt:ScopeDef>Fixed</tt:ScopeDef><tt:ScopeItem>{}</tt:ScopeItem></tds:Scopes>",
                        escape(scope)
                    );
                    s
                });
            ok(format!("<tds:GetScopesResponse>{scopes}</tds:GetScopesResponse>"))
        }
        "GetHostname" => ok(format!(
            "<tds:GetHostnameResponse><tds:HostnameInformation><tt:FromDHCP>false</tt:FromDHCP><tt:Name>{}</tt:Name></tds:HostnameInformation></tds:GetHostnameResponse>",
            escape(&camera.path)
        )),
        "GetServiceCapabilities" => ok(
            "<tds:GetServiceCapabilitiesResponse><tds:Capabilities><tds:Network IPFilter=\"false\" ZeroConfiguration=\"false\" IPVersion6=\"false\" DynDNS=\"false\"/><tds:Security UsernameToken=\"true\" HttpDigest=\"true\"/><tds:System DiscoveryResolve=\"false\" DiscoveryBye=\"false\" RemoteDiscovery=\"false\" SystemBackup=\"false\" SystemLogging=\"false\" FirmwareUpgrade=\"false\"/></tds:Capabilities></tds:GetServiceCapabilitiesResponse>".to_owned(),
        ),
        "GetProfiles" => {
            let all: String = PROFILES.iter().fold(String::new(), |mut s, token| {
                let _ = write!(s, r#"<trt:Profiles token="{token}" fixed="true">{}</trt:Profiles>"#, profile(camera, token));
                s
            });
            ok(format!("<trt:GetProfilesResponse>{all}</trt:GetProfilesResponse>"))
        }
        "GetProfile" => {
            let token = element(xml, "ProfileToken").unwrap_or("main");
            let Some(token) = PROFILES.iter().find(|t| **t == token) else {
                return fault(StatusCode::BAD_REQUEST, "Sender", "NoProfile", "No such profile");
            };
            ok(format!(
                r#"<trt:GetProfileResponse><trt:Profile token="{token}" fixed="true">{}</trt:Profile></trt:GetProfileResponse>"#,
                profile(camera, token)
            ))
        }
        "GetVideoSources" => {
            let (w, h, _) = picture(camera);
            ok(format!(
                r#"<trt:GetVideoSourcesResponse><trt:VideoSources token="video_source"><tt:Framerate>30</tt:Framerate><tt:Resolution><tt:Width>{w}</tt:Width><tt:Height>{h}</tt:Height></tt:Resolution></trt:VideoSources></trt:GetVideoSourcesResponse>"#
            ))
        }
        "GetVideoSourceConfigurations" | "GetVideoSourceConfiguration" => {
            let (w, h, _) = picture(camera);
            let inner = format!(
                r#"token="video_source_config"><tt:Name>video source</tt:Name><tt:UseCount>2</tt:UseCount><tt:SourceToken>video_source</tt:SourceToken><tt:Bounds x="0" y="0" width="{w}" height="{h}"/>"#
            );
            if action == "GetVideoSourceConfigurations" {
                ok(format!("<trt:GetVideoSourceConfigurationsResponse><trt:Configurations {inner}</trt:Configurations></trt:GetVideoSourceConfigurationsResponse>"))
            } else {
                ok(format!("<trt:GetVideoSourceConfigurationResponse><trt:Configuration {inner}</trt:Configuration></trt:GetVideoSourceConfigurationResponse>"))
            }
        }
        "GetVideoEncoderConfigurations" => {
            let (w, h, h264) = picture(camera);
            let all: String = PROFILES.iter().fold(String::new(), |mut s, token| {
                s.push_str(
                    &encoder(camera, token, w, h, h264)
                        .replace("<tt:VideoEncoderConfiguration ", "<trt:Configurations ")
                        .replace("</tt:VideoEncoderConfiguration>", "</trt:Configurations>"),
                );
                s
            });
            ok(format!("<trt:GetVideoEncoderConfigurationsResponse>{all}</trt:GetVideoEncoderConfigurationsResponse>"))
        }
        "GetVideoEncoderConfiguration" => {
            let (w, h, h264) = picture(camera);
            let token = if element(xml, "ConfigurationToken") == Some("encoder_sub") { "sub" } else { "main" };
            ok(format!(
                "<trt:GetVideoEncoderConfigurationResponse>{}</trt:GetVideoEncoderConfigurationResponse>",
                encoder(camera, token, w, h, h264)
                    .replace("<tt:VideoEncoderConfiguration ", "<trt:Configuration ")
                    .replace("</tt:VideoEncoderConfiguration>", "</trt:Configuration>")
            ))
        }
        "GetVideoEncoderConfigurationOptions" => {
            // One stream, as the camera sends it: nothing to choose.
            let (w, h, h264) = picture(camera);
            ok(format!(
                "<trt:GetVideoEncoderConfigurationOptionsResponse><trt:Options><tt:QualityRange><tt:Min>4</tt:Min><tt:Max>4</tt:Max></tt:QualityRange><tt:H264><tt:ResolutionsAvailable><tt:Width>{w}</tt:Width><tt:Height>{h}</tt:Height></tt:ResolutionsAvailable><tt:GovLengthRange><tt:Min>30</tt:Min><tt:Max>30</tt:Max></tt:GovLengthRange><tt:FrameRateRange><tt:Min>30</tt:Min><tt:Max>30</tt:Max></tt:FrameRateRange><tt:EncodingIntervalRange><tt:Min>1</tt:Min><tt:Max>1</tt:Max></tt:EncodingIntervalRange><tt:H264ProfilesSupported>{h264}</tt:H264ProfilesSupported></tt:H264></trt:Options></trt:GetVideoEncoderConfigurationOptionsResponse>"
            ))
        }
        "GetAudioSources" => ok("<trt:GetAudioSourcesResponse/>".to_owned()),
        "GetAudioEncoderConfigurations" => ok("<trt:GetAudioEncoderConfigurationsResponse/>".to_owned()),
        "GetStreamUri" => ok(format!("<trt:GetStreamUriResponse>{}</trt:GetStreamUriResponse>", uri(&rtsp))),
        "GetSnapshotUri" => ok(format!(
            "<trt:GetSnapshotUriResponse>{}</trt:GetSnapshotUriResponse>",
            uri(&format!("{base}/onvif/snapshot.jpg"))
        )),
        _ => {
            tracing::info!(device = %camera.id, action, "onvif: action not supported");
            fault(StatusCode::BAD_REQUEST, "Receiver", "ActionNotSupported", "Optional action not implemented")
        }
    }
}

/// The scopes the device announces (and answers `GetScopes` with).
#[must_use]
pub fn scopes(camera: &Device) -> Vec<String> {
    let name: String = camera
        .name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    vec![
        "onvif://www.onvif.org/type/video_encoder".to_owned(),
        "onvif://www.onvif.org/type/Network_Video_Transmitter".to_owned(),
        "onvif://www.onvif.org/Profile/Streaming".to_owned(),
        "onvif://www.onvif.org/hardware/Moli".to_owned(),
        format!("onvif://www.onvif.org/name/{name}"),
    ]
}

async fn service(State(camera): State<Arc<Device>>, headers: HeaderMap, body: Bytes) -> Response {
    let Ok(xml) = std::str::from_utf8(&body) else {
        return fault(StatusCode::BAD_REQUEST, "Sender", "WellFormed", "Not UTF-8");
    };
    let Some(action) = action(xml) else {
        return fault(
            StatusCode::BAD_REQUEST,
            "Sender",
            "WellFormed",
            "No SOAP action",
        );
    };
    let Some(host) = host_of(&headers) else {
        return fault(
            StatusCode::BAD_REQUEST,
            "Sender",
            "InvalidArgVal",
            "No host",
        );
    };
    let password = camera.shared.password();
    let now = jiff::Timestamp::now().as_second();
    let ws = ws_security(xml, &camera.shared.user, password.as_deref(), now);
    let (http, _) = http_credentials(&headers, "POST", &camera.shared);
    if ws == Credentials::Wrong || http == Credentials::Wrong {
        tracing::warn!(device = %camera.id, action, "onvif: wrong credentials");
        // Slows down guessing.
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        return not_authorized();
    }
    if camera.shared.hub.protected_room(&camera.id).is_some() {
        return not_authorized();
    }
    tracing::debug!(device = %camera.id, action, "onvif: request");
    answer(&camera, action, xml, &host)
}

async fn snapshot(State(camera): State<Arc<Device>>, headers: HeaderMap) -> Response {
    let (credentials, nonce) = http_credentials(&headers, "GET", &camera.shared);
    match credentials {
        Credentials::Proved => {}
        Credentials::Absent | Credentials::Wrong => {
            if credentials == Credentials::Wrong {
                tracing::warn!(device = %camera.id, "onvif: wrong password for the image");
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
            let mut r = StatusCode::UNAUTHORIZED.into_response();
            if let Ok(v) = HeaderValue::from_str(&http_digest::http_challenge(&nonce, false)) {
                r.headers_mut().insert(header::WWW_AUTHENTICATE, v);
            }
            return r;
        }
    }
    if camera.shared.hub.protected_room(&camera.id).is_some() {
        return StatusCode::FORBIDDEN.into_response();
    }
    match camera.shared.hub.camera_image(&camera.id).await {
        Ok(image) => {
            let mut r = image.bytes.into_response();
            if let Ok(v) = HeaderValue::from_str(&image.content_type) {
                r.headers_mut().insert(header::CONTENT_TYPE, v);
            }
            r
        }
        Err(e) => {
            tracing::info!(device = %camera.id, error = %e, "onvif: no image");
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
    }
}

/// Serves one camera as an ONVIF device on `listener`.
pub fn serve(
    camera: Arc<Device>,
    listener: tokio::net::TcpListener,
    cancel: tokio_util::sync::CancellationToken,
) -> tokio::task::JoinHandle<()> {
    let app = axum::Router::new()
        .route("/onvif/device_service", post(service))
        .route("/onvif/media_service", post(service))
        .route("/onvif/snapshot.jpg", get(snapshot))
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .with_state(Arc::clone(&camera));
    tokio::spawn(async move {
        let result = axum::serve(listener, app)
            .with_graceful_shutdown(async move { cancel.cancelled().await })
            .await;
        if let Err(e) = result {
            tracing::warn!(device = %camera.id, error = %e, "onvif: the device stopped");
        }
    })
}

/// The address to reach `peer` from (the one to announce to it).
pub fn local_for(peer: SocketAddr) -> Option<std::net::IpAddr> {
    let probe = std::net::UdpSocket::bind(("0.0.0.0", 0)).ok()?;
    probe.connect(peer).ok()?;
    Some(probe.local_addr().ok()?.ip())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elements_are_found_whatever_their_prefix() {
        let xml = r#"<?xml version="1.0"?><s:Envelope xmlns:s="x"><s:Header><wsse:Security><wsse:UsernameToken><wsse:Username>moli</wsse:Username></wsse:UsernameToken></wsse:Security></s:Header><s:Body xmlns:trt="y"><trt:GetStreamUri><trt:StreamSetup/><trt:ProfileToken>sub</trt:ProfileToken></trt:GetStreamUri></s:Body></s:Envelope>"#;
        assert_eq!(action(xml), Some("GetStreamUri"));
        assert_eq!(element(xml, "ProfileToken"), Some("sub"));
        assert_eq!(element(xml, "Username"), Some("moli"));
        assert_eq!(element(xml, "StreamSetup"), Some(""));
        assert_eq!(element(xml, "Missing"), None);
        // No prefix at all.
        let plain = "<Envelope><Body><GetProfiles/></Body></Envelope>";
        assert_eq!(action(plain), Some("GetProfiles"));
    }

    #[test]
    fn escape_keeps_xml_safe() {
        assert_eq!(escape(r#"a<b>&"c'"#), "a&lt;b&gt;&amp;&quot;c&apos;");
    }

    #[test]
    fn base64_round_trips() {
        for bytes in [&b""[..], b"f", b"fo", b"foo", b"foob", &[0xFF, 0x00, 0x7F]] {
            assert_eq!(
                base64_decode(&crate::connection::base64(bytes)).unwrap(),
                bytes
            );
        }
        assert!(base64_decode("a$b").is_none());
    }

    fn token(user: &str, password: &str, created: &str, nonce: &[u8]) -> String {
        let mut input = nonce.to_vec();
        input.extend_from_slice(created.as_bytes());
        input.extend_from_slice(password.as_bytes());
        let digest = crate::connection::base64(digest(&SHA1_FOR_LEGACY_USE_ONLY, &input).as_ref());
        format!(
            r#"<s:Envelope><s:Header><Security><UsernameToken><Username>{user}</Username><Password Type="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-username-token-profile-1.0#PasswordDigest">{digest}</Password><Nonce EncodingType="x">{}</Nonce><Created>{created}</Created></UsernameToken></Security></s:Header><s:Body><GetProfiles/></s:Body></s:Envelope>"#,
            crate::connection::base64(nonce)
        )
    }

    #[test]
    fn a_ws_security_digest_is_checked() {
        // The ONVIF Core specification's example (§5.12.3.1): password
        // "userpassword".
        let nonce = base64_decode("LKqI6G/AikKCQrN0zqZFlg==").unwrap();
        let created = "2010-09-16T07:50:45Z";
        let example = token("user", "userpassword", created, &nonce);
        assert!(
            example.contains("tuOSpGlFlIXsozq4HFNeeGeFLEI="),
            "the spec's digest"
        );
        let at = created.parse::<jiff::Timestamp>().unwrap().as_second();
        assert_eq!(
            ws_security(&example, "user", Some("userpassword"), at),
            Credentials::Proved
        );
        assert_eq!(
            ws_security(&example, "user", Some("other"), at),
            Credentials::Wrong
        );
        assert_eq!(
            ws_security(&example, "admin", Some("userpassword"), at),
            Credentials::Wrong
        );
        // A day later: an old token.
        assert_eq!(
            ws_security(&example, "user", Some("userpassword"), at + 86_400),
            Credentials::Wrong
        );
        // No password set in Moli: nobody passes.
        assert_eq!(ws_security(&example, "user", None, at), Credentials::Wrong);
        // No token: nothing claimed.
        assert_eq!(
            ws_security(
                "<Envelope><Body><GetProfiles/></Body></Envelope>",
                "user",
                None,
                at
            ),
            Credentials::Absent
        );
    }

    #[test]
    fn a_ws_security_text_password_is_checked() {
        let xml = r#"<Envelope><Header><UsernameToken><Username>moli</Username><Password Type="...#PasswordText">pw</Password></UsernameToken></Header><Body><GetProfiles/></Body></Envelope>"#;
        assert_eq!(ws_security(xml, "moli", Some("pw"), 0), Credentials::Proved);
        assert_eq!(ws_security(xml, "moli", Some("px"), 0), Credentials::Wrong);
    }

    #[test]
    fn uuids_are_stable_and_shaped() {
        let a = uuid_of("homekit:doorbell");
        assert_eq!(a, uuid_of("homekit:doorbell"));
        assert_ne!(a, uuid_of("homekit:other"));
        assert_eq!(a.len(), 36);
        assert_eq!(&a[14..15], "5");
    }

    #[test]
    fn the_host_is_taken_without_its_port_and_only_if_plain() {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, HeaderValue::from_static("192.168.1.2:8899"));
        assert_eq!(host_of(&h).as_deref(), Some("192.168.1.2"));
        h.insert(header::HOST, HeaderValue::from_static("[fe80::1]:8899"));
        assert_eq!(host_of(&h).as_deref(), Some("[fe80::1]"));
        h.insert(header::HOST, HeaderValue::from_static("evil\"<x>"));
        assert_eq!(host_of(&h), None);
    }
}
