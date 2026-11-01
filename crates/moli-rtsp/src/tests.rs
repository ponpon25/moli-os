//! A client against a real surface and fake cameras: the password, the
//! description, the interleaved video, the guard, and a camera whose
//! stream ends while the client watches.

use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use md5::{Digest as _, Md5};
use moli_core::{Access, Device, DeviceId, InstanceId, Kind, PointSpec, Semantic, Value};
use moli_net::h264::Depacketizer;
use moli_runtime::media::{LiveInfo, LiveItem, LiveSource, LiveStream, LiveWant, VideoFrame};
use moli_runtime::{BoxFuture, Driver, DriverCtx, Hub, HubOptions, MasterKey, spawn_driver};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

use crate::{Camera, Config};

const SPS: [u8; 4] = [0x67, 0x64, 0x00, 0x28];
const PPS: [u8; 3] = [0x68, 0xEB, 0xEC];
const PASSWORD: &str = "correct horse";

/// A stream of 25 frames a second, a key frame every 10, that ends after
/// `length` frames. Its clock starts somewhere new each time.
struct FakeStream {
    info: LiveInfo,
    n: u32,
    length: u32,
    base: u32,
}

impl LiveStream for FakeStream {
    fn info(&self) -> &LiveInfo {
        &self.info
    }

    fn next(&mut self) -> BoxFuture<'_, anyhow::Result<LiveItem>> {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(40)).await;
            if self.n >= self.length {
                anyhow::bail!("the fake camera stops");
            }
            let n = self.n;
            self.n += 1;
            let key = n.is_multiple_of(10);
            let picture: Vec<u8> = std::iter::once(if key { 0x65 } else { 0x41 })
                .chain(std::iter::repeat_n(0x80, if key { 3000 } else { 300 }))
                .collect();
            Ok(LiveItem::Video(VideoFrame {
                timestamp: self.base.wrapping_add(n * 3600),
                nals: if key {
                    vec![SPS.to_vec(), PPS.to_vec(), picture]
                } else {
                    vec![picture]
                },
                keyframe: key,
            }))
        })
    }
}

#[derive(Debug)]
struct FakeLive {
    length: u32,
    opened: Arc<AtomicU32>,
}

impl LiveSource for FakeLive {
    fn open(&self, _want: LiveWant) -> BoxFuture<'_, anyhow::Result<Box<dyn LiveStream>>> {
        let n = self.opened.fetch_add(1, Ordering::SeqCst);
        let length = self.length;
        Box::pin(async move {
            Ok(Box::new(FakeStream {
                info: LiveInfo {
                    width: 1280,
                    height: 720,
                    sps: SPS.to_vec(),
                    pps: PPS.to_vec(),
                    audio_rate: None,
                },
                n: 0,
                length,
                base: 1_000_000u32.wrapping_mul(n + 1),
            }) as Box<dyn LiveStream>)
        })
    }
}

/// `door` (its streams end after 15 frames) and `nursery`, in a protected room.
struct FakeCameras(Arc<AtomicU32>);

impl Driver for FakeCameras {
    fn kind(&self) -> &'static str {
        "fake"
    }

    fn run<'a>(&'a self, ctx: &'a mut DriverCtx) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            for (native, room) in [("door", None), ("nursery", Some("Chambre"))] {
                let d = Device {
                    id: ctx.device_id(native),
                    instance: ctx.instance().clone(),
                    native_name: native.into(),
                    manufacturer: None,
                    model: None,
                    description: None,
                    native_room: room.map(Into::into),
                    members: Vec::new(),
                    points: vec![PointSpec {
                        key: "motion".into(),
                        label: "motion".into(),
                        kind: Kind::Binary,
                        access: Access {
                            read: true,
                            write: false,
                        },
                        unit: None,
                        semantic: Semantic::Occupancy,
                    }],
                };
                let id = d.id.clone();
                ctx.upsert_device(d);
                ctx.provide_live(
                    &id,
                    Arc::new(FakeLive {
                        length: 15,
                        opened: Arc::clone(&self.0),
                    }),
                );
                ctx.set_state(&id, "motion", Value::Bool(false));
            }
            ctx.ready();
            while let Some(cmd) = ctx.next_command().await {
                cmd.reply(Ok(()));
            }
            Ok(())
        })
    }
}

struct Setup {
    port: u16,
    hub: Hub,
    opened: Arc<AtomicU32>,
    _cancel: tokio_util::sync::DropGuard,
}

async fn setup(name: &str, password: Option<&str>) -> Result<Setup, crate::StartError> {
    let dir = std::env::temp_dir().join(format!("moli-rtsp-it-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let hub = Hub::new(HubOptions {
        secrets: Some((dir.join("secrets.enc"), MasterKey::generate().unwrap())),
        guard: moli_runtime::guard::GuardPolicy {
            protected_rooms: vec!["Chambre".into()],
            quiet_hours: None,
            protect_unassigned: false,
            dashboard: moli_runtime::guard::Dashboard::Trusted,
        },
        ..HubOptions::default()
    })
    .unwrap();
    if let Some(p) = password {
        hub.store_secret(
            &InstanceId::from(crate::NAMESPACE),
            crate::SECRET_PASSWORD,
            p,
        )
        .await
        .unwrap();
    }
    let cancel = CancellationToken::new();
    let opened = Arc::new(AtomicU32::new(0));
    spawn_driver(
        &hub,
        InstanceId::from("fake"),
        Arc::new(FakeCameras(Arc::clone(&opened))),
        cancel.clone(),
    );
    for _ in 0..200 {
        if hub.has_live(&DeviceId::from("fake:nursery")) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let config = Config {
        port: 0,
        user: "moli".into(),
        cameras: vec![
            Camera {
                path: "door".into(),
                device: DeviceId::from("fake:door"),
            },
            Camera {
                path: "nursery".into(),
                device: DeviceId::from("fake:nursery"),
            },
        ],
    };
    let (address, _task) = crate::start(hub.clone(), config, cancel.clone()).await?;
    Ok(Setup {
        port: address.port(),
        hub,
        opened,
        _cancel: cancel.drop_guard(),
    })
}

fn md5_hex(s: &str) -> String {
    Md5::digest(s.as_bytes())
        .iter()
        .fold(String::new(), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

/// A plain RTSP client, as ffmpeg talks.
struct Client {
    reader: BufReader<tokio::net::tcp::OwnedReadHalf>,
    writer: tokio::net::tcp::OwnedWriteHalf,
    cseq: u32,
    nonce: Option<String>,
    password: String,
}

#[derive(Debug)]
struct Response {
    code: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Response {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

impl Client {
    async fn connect(port: u16, password: &str) -> Self {
        let stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let (r, w) = stream.into_split();
        Self {
            reader: BufReader::new(r),
            writer: w,
            cseq: 0,
            nonce: None,
            password: password.into(),
        }
    }

    async fn send(&mut self, method: &str, uri: &str, extra: &str) -> Response {
        self.cseq += 1;
        let mut head = format!("{method} {uri} RTSP/1.0\r\nCSeq: {}\r\n{extra}", self.cseq);
        if let Some(nonce) = &self.nonce {
            let ha1 = md5_hex(&format!("moli:Moli:{}", self.password));
            let ha2 = md5_hex(&format!("{method}:{uri}"));
            let r = md5_hex(&format!("{ha1}:{nonce}:{ha2}"));
            let _ = write!(
                head,
                "Authorization: Digest username=\"moli\", realm=\"Moli\", nonce=\"{nonce}\", uri=\"{uri}\", response=\"{r}\"\r\n"
            );
        }
        head.push_str("\r\n");
        self.writer.write_all(head.as_bytes()).await.unwrap();
        let response = self.response().await;
        assert_eq!(
            response.header("cseq"),
            Some(self.cseq.to_string().as_str())
        );
        response
    }

    async fn response(&mut self) -> Response {
        let mut line = String::new();
        // Interleaved packets may come before the reply: skip them.
        loop {
            let first = self.reader.fill_buf().await.unwrap()[0];
            if first != b'$' {
                break;
            }
            let _ = self.packet().await;
        }
        self.reader.read_line(&mut line).await.unwrap();
        let code = line.split(' ').nth(1).unwrap().parse().unwrap();
        let mut headers = Vec::new();
        loop {
            let mut l = String::new();
            self.reader.read_line(&mut l).await.unwrap();
            let l = l.trim_end();
            if l.is_empty() {
                break;
            }
            let (n, v) = l.split_once(':').unwrap();
            headers.push((n.trim().to_owned(), v.trim().to_owned()));
        }
        let len: usize = headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case("content-length"))
            .map_or(0, |(_, v)| v.parse().unwrap());
        let mut body = vec![0; len];
        self.reader.read_exact(&mut body).await.unwrap();
        Response {
            code,
            headers,
            body: String::from_utf8(body).unwrap(),
        }
    }

    /// One interleaved packet: (channel, RTP packet).
    async fn packet(&mut self) -> (u8, Vec<u8>) {
        let mut head = [0u8; 4];
        self.reader.read_exact(&mut head).await.unwrap();
        assert_eq!(head[0], b'$');
        let mut p = vec![0; usize::from(u16::from_be_bytes([head[2], head[3]]))];
        self.reader.read_exact(&mut p).await.unwrap();
        (head[1], p)
    }

    /// Asks once without a password to get the nonce, as players do.
    async fn authenticate(&mut self, uri: &str) {
        let r = self.send("DESCRIBE", uri, "").await;
        assert_eq!(r.code, 401);
        let challenge = r.header("www-authenticate").unwrap();
        let nonce = challenge
            .split("nonce=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        self.nonce = Some(nonce.to_owned());
    }

    async fn play(&mut self, uri: &str) -> Response {
        self.authenticate(uri).await;
        let r = self
            .send("DESCRIBE", uri, "Accept: application/sdp\r\n")
            .await;
        assert_eq!(r.code, 200, "{r:?}");
        let r = self
            .send(
                "SETUP",
                &format!("{uri}/track1"),
                "Transport: RTP/AVP/TCP;unicast;interleaved=0-1\r\n",
            )
            .await;
        assert_eq!(r.code, 200, "{r:?}");
        let session = r
            .header("session")
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        self.send("PLAY", uri, &format!("Session: {session}\r\n"))
            .await
    }
}

#[tokio::test]
async fn without_a_password_nobody_gets_in_until_one_is_set() {
    let s = setup("nopassword", None).await.unwrap();
    let uri = format!("rtsp://127.0.0.1:{}/door", s.port);
    let mut c = Client::connect(s.port, "").await;
    c.authenticate(&uri).await;
    assert_eq!(c.send("DESCRIBE", &uri, "").await.code, 401);
    assert_eq!(s.opened.load(Ordering::SeqCst), 0);
    // Set now: it counts at once, without a restart.
    s.hub
        .store_secret(
            &InstanceId::from(crate::NAMESPACE),
            crate::SECRET_PASSWORD,
            PASSWORD,
        )
        .await
        .unwrap();
    let mut c = Client::connect(s.port, PASSWORD).await;
    assert_eq!(c.play(&uri).await.code, 200);
}

#[tokio::test]
async fn a_player_sees_the_camera_and_nothing_else() {
    let s = setup("play", Some(PASSWORD)).await.unwrap();
    let uri = format!("rtsp://127.0.0.1:{}/door", s.port);

    // OPTIONS is open; the rest wants the password.
    let mut c = Client::connect(s.port, PASSWORD).await;
    let r = c.send("OPTIONS", &uri, "").await;
    assert_eq!(r.code, 200);
    assert!(r.header("public").unwrap().contains("DESCRIBE"));

    // The description.
    c.authenticate(&uri).await;
    let r = c.send("DESCRIBE", &uri, "").await;
    assert_eq!(r.code, 200);
    assert!(r.body.contains("a=rtpmap:96 H264/90000"));
    assert!(r.body.contains("sprop-parameter-sets=Z2QAKA==,aOvs"));
    assert_eq!(r.header("content-base"), Some(format!("{uri}/").as_str()));

    // UDP is refused, TCP accepted.
    let r = c
        .send(
            "SETUP",
            &format!("{uri}/track1"),
            "Transport: RTP/AVP;unicast;client_port=5000-5001\r\n",
        )
        .await;
    assert_eq!(r.code, 461);
    let r = c
        .send(
            "SETUP",
            &format!("{uri}/track1"),
            "Transport: RTP/AVP/TCP;unicast;interleaved=0-1\r\n",
        )
        .await;
    assert_eq!(r.code, 200);
    assert_eq!(
        r.header("transport"),
        Some("RTP/AVP/TCP;unicast;interleaved=0-1")
    );
    let session = r
        .header("session")
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let r = c
        .send("PLAY", &uri, &format!("Session: {session}\r\n"))
        .await;
    assert_eq!(r.code, 200);
    let rtp_info = r.header("rtp-info").unwrap().to_owned();
    let rtptime: u32 = rtp_info.split("rtptime=").nth(1).unwrap().parse().unwrap();
    let seq: u16 = rtp_info
        .split("seq=")
        .nth(1)
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .parse()
        .unwrap();

    // The video: starts on a key frame, at the announced seq and time, and
    // goes on past the end of the first stream (15 frames) with time moving
    // forward.
    let mut depacketizer = Depacketizer::new();
    let mut units = Vec::new();
    let mut first_packet = true;
    let mut last_ts: Option<u32> = None;
    let mut last_seq = seq.wrapping_sub(1);
    tokio::time::timeout(Duration::from_secs(10), async {
        while units.len() < 40 {
            let (channel, p) = c.packet().await;
            assert_eq!(channel, 0);
            let pseq = u16::from_be_bytes([p[2], p[3]]);
            let ts = u32::from_be_bytes([p[4], p[5], p[6], p[7]]);
            if first_packet {
                assert_eq!(pseq, seq);
                assert_eq!(ts, rtptime);
                first_packet = false;
            }
            assert_eq!(pseq, last_seq.wrapping_add(1), "no packet lost");
            last_seq = pseq;
            if let Some(last) = last_ts {
                assert!(
                    ts.wrapping_sub(last).cast_signed() >= 0,
                    "time never goes back"
                );
            }
            last_ts = Some(ts);
            units.extend(depacketizer.push(&p));
        }
    })
    .await
    .expect("40 frames in time");
    assert!(units[0].keyframe);
    assert!(
        s.opened.load(Ordering::SeqCst) >= 3,
        "the stream was opened again"
    );

    // Leaving.
    let r = c
        .send("TEARDOWN", &uri, &format!("Session: {session}\r\n"))
        .await;
    assert_eq!(r.code, 200);
}

#[tokio::test]
async fn wrong_passwords_unknown_paths_and_protected_rooms_are_refused() {
    let s = setup("refused", Some(PASSWORD)).await.unwrap();
    let base = format!("rtsp://127.0.0.1:{}", s.port);

    let mut c = Client::connect(s.port, PASSWORD).await;
    c.authenticate(&format!("{base}/door")).await;
    assert_eq!(
        c.send("DESCRIBE", &format!("{base}/garage"), "").await.code,
        404
    );
    assert_eq!(
        c.send("DESCRIBE", &format!("{base}/nursery"), "")
            .await
            .code,
        403
    );
    assert_eq!(c.send("PLAY", &format!("{base}/door"), "").await.code, 455);

    // Three wrong passwords and the connection is closed.
    let mut c = Client::connect(s.port, "guess").await;
    c.authenticate(&format!("{base}/door")).await;
    assert_eq!(
        c.send("DESCRIBE", &format!("{base}/door"), "").await.code,
        401
    );
    assert_eq!(
        c.send("DESCRIBE", &format!("{base}/door"), "").await.code,
        401
    );
    assert_eq!(
        c.send("DESCRIBE", &format!("{base}/door"), "").await.code,
        401
    );
    let mut rest = Vec::new();
    let n = tokio::time::timeout(Duration::from_secs(2), c.reader.read_to_end(&mut rest))
        .await
        .expect("closed")
        .unwrap();
    assert_eq!(n, 0);
    assert_eq!(
        s.opened.load(Ordering::SeqCst),
        0,
        "no camera opened for a stranger"
    );

    // The right password plays.
    let mut c = Client::connect(s.port, PASSWORD).await;
    assert_eq!(c.play(&format!("{base}/door")).await.code, 200);
    let (channel, _) = c.packet().await;
    assert_eq!(channel, 0);
}

#[tokio::test]
async fn a_player_answers_on_a_new_connection_with_the_nonce_it_was_given() {
    // VLC: the challenge on one connection, the password asked, the answer
    // on the next one.
    let s = setup("vlc", Some(PASSWORD)).await.unwrap();
    let uri = format!("rtsp://127.0.0.1:{}/door", s.port);
    let mut first = Client::connect(s.port, PASSWORD).await;
    first.authenticate(&uri).await;
    drop(first.writer);
    let mut second = Client::connect(s.port, PASSWORD).await;
    second.nonce = first.nonce.clone();
    assert_eq!(second.send("DESCRIBE", &uri, "").await.code, 200);
}
