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
                ctx.provide_snapshots(&id, Arc::new(FakeJpeg));
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
    onvif: Vec<(String, std::net::SocketAddr)>,
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
                name: Some("Front door".into()),
                onvif_port: Some(0),
            },
            Camera {
                path: "nursery".into(),
                device: DeviceId::from("fake:nursery"),
                name: None,
                onvif_port: Some(0),
            },
        ],
        discovery: false,
    };
    let started = crate::start(hub.clone(), config, cancel.clone()).await?;
    Ok(Setup {
        port: started.rtsp.port(),
        onvif: started.onvif,
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

    // A multicast-only client is refused; TCP is taken.
    let r = c
        .send(
            "SETUP",
            &format!("{uri}/track1"),
            "Transport: RTP/AVP;multicast;port=5000-5001\r\n",
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
    let mut reports = 0;
    tokio::time::timeout(Duration::from_secs(10), async {
        while units.len() < 40 {
            let (channel, p) = c.packet().await;
            if channel == 1 {
                assert_eq!(p[1], 200, "RTCP on the second channel: a sender report");
                reports += 1;
                continue;
            }
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
    loop {
        let (channel, _) = c.packet().await;
        if channel == 0 {
            break;
        }
    }
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

/// One camera stream shared by every viewer (as Moli's HomeKit doorbell):
/// a frame every 33 ms, a key frame every 30, and a viewer let go after ten
/// minutes.
#[derive(Debug)]
struct SharedLive {
    frames: tokio::sync::broadcast::Sender<VideoFrame>,
    opened: Arc<AtomicU32>,
}

struct SharedView {
    info: LiveInfo,
    rx: tokio::sync::broadcast::Receiver<VideoFrame>,
    waiting_key: bool,
    since: tokio::time::Instant,
}

impl LiveStream for SharedView {
    fn info(&self) -> &LiveInfo {
        &self.info
    }

    fn next(&mut self) -> BoxFuture<'_, anyhow::Result<LiveItem>> {
        Box::pin(async move {
            loop {
                if self.since.elapsed() > Duration::from_secs(600) {
                    anyhow::bail!("watched for 10 minutes: stopped");
                }
                let frame = self.rx.recv().await?;
                if self.waiting_key && !frame.keyframe {
                    continue;
                }
                self.waiting_key = false;
                return Ok(LiveItem::Video(frame));
            }
        })
    }
}

impl LiveSource for SharedLive {
    fn open(&self, _want: LiveWant) -> BoxFuture<'_, anyhow::Result<Box<dyn LiveStream>>> {
        self.opened.fetch_add(1, Ordering::SeqCst);
        let rx = self.frames.subscribe();
        Box::pin(async move {
            Ok(Box::new(SharedView {
                info: LiveInfo {
                    width: 1280,
                    height: 720,
                    sps: SPS.to_vec(),
                    pps: PPS.to_vec(),
                    audio_rate: None,
                },
                rx,
                waiting_key: true,
                since: tokio::time::Instant::now(),
            }) as Box<dyn LiveStream>)
        })
    }
}

struct SharedCamera(Arc<AtomicU32>);

impl Driver for SharedCamera {
    fn kind(&self) -> &'static str {
        "fake"
    }

    fn run<'a>(&'a self, ctx: &'a mut DriverCtx) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            let d = Device {
                id: ctx.device_id("bell"),
                instance: ctx.instance().clone(),
                native_name: "bell".into(),
                manufacturer: None,
                model: None,
                description: None,
                native_room: None,
                members: Vec::new(),
                points: Vec::new(),
            };
            let id = d.id.clone();
            ctx.upsert_device(d);
            let (frames, _) = tokio::sync::broadcast::channel(64);
            ctx.provide_live(
                &id,
                Arc::new(SharedLive {
                    frames: frames.clone(),
                    opened: Arc::clone(&self.0),
                }),
            );
            ctx.ready();
            tokio::spawn(async move {
                let mut n: u32 = 0;
                loop {
                    tokio::time::sleep(Duration::from_millis(33)).await;
                    let key = n.is_multiple_of(30);
                    let _ = frames.send(VideoFrame {
                        timestamp: 7_000_000u32.wrapping_add(n * 3000),
                        nals: vec![vec![if key { 0x65 } else { 0x41 }, 0x80]],
                        keyframe: key,
                    });
                    n += 1;
                }
            });
            while let Some(cmd) = ctx.next_command().await {
                cmd.reply(Ok(()));
            }
            Ok(())
        })
    }
}

#[tokio::test(start_paused = true)]
async fn a_shared_stream_is_renewed_without_a_gap_or_a_frame_twice() {
    let hub = Hub::new(HubOptions::default()).unwrap();
    let opened = Arc::new(AtomicU32::new(0));
    let cancel = CancellationToken::new();
    let _guard = cancel.clone().drop_guard();
    spawn_driver(
        &hub,
        InstanceId::from("fake"),
        Arc::new(SharedCamera(Arc::clone(&opened))),
        cancel,
    );
    let bell = DeviceId::from("fake:bell");
    for _ in 0..200 {
        if hub.has_live(&bell) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let first = crate::feed::open(&hub, &bell).await.unwrap();
    let mut rx = crate::feed::spawn(hub.clone(), bell, first, 1000);
    // 21 minutes: two renewals, and the ten-minute limit never reached.
    let frames = 21 * 60 * 30;
    let mut last: Option<u32> = None;
    for i in 0..frames {
        let frame = rx.recv().await.expect("the feed goes on");
        if let Some(last) = last {
            assert_eq!(
                frame.timestamp.wrapping_sub(last),
                3000,
                "frame {i}: one frame after the other, none missed, none twice"
            );
        } else {
            assert_eq!(frame.timestamp, 1000);
            assert!(frame.keyframe);
        }
        last = Some(frame.timestamp);
    }
    assert_eq!(
        opened.load(Ordering::SeqCst),
        3,
        "opened once, renewed twice"
    );
}

#[tokio::test]
async fn a_client_in_udp_gets_the_video_on_its_ports_and_sender_reports() {
    let s = setup("udp", Some(PASSWORD)).await.unwrap();
    let uri = format!("rtsp://127.0.0.1:{}/door", s.port);
    let video = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let reports = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let (video_port, report_port) = (
        video.local_addr().unwrap().port(),
        reports.local_addr().unwrap().port(),
    );
    let mut c = Client::connect(s.port, PASSWORD).await;
    c.authenticate(&uri).await;
    assert_eq!(c.send("DESCRIBE", &uri, "").await.code, 200);
    let r = c
        .send(
            "SETUP",
            &format!("{uri}/track1"),
            &format!("Transport: RTP/AVP;unicast;client_port={video_port}-{report_port}\r\n"),
        )
        .await;
    assert_eq!(r.code, 200, "{r:?}");
    let transport = r.header("transport").unwrap();
    assert!(transport.contains(&format!("client_port={video_port}-{report_port}")));
    assert!(transport.contains("server_port="));
    let session = r
        .header("session")
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    assert_eq!(
        c.send("PLAY", &uri, &format!("Session: {session}\r\n"))
            .await
            .code,
        200
    );
    let mut depacketizer = Depacketizer::new();
    let mut buf = vec![0u8; 2000];
    let units = tokio::time::timeout(Duration::from_secs(5), async {
        let mut units = Vec::new();
        while units.len() < 5 {
            let (n, _) = video.recv_from(&mut buf).await.unwrap();
            units.extend(depacketizer.push(&buf[..n]));
        }
        units
    })
    .await
    .expect("video in UDP");
    assert!(units[0].keyframe);
    // The first sender report comes at once.
    let (n, _) = tokio::time::timeout(Duration::from_secs(6), reports.recv_from(&mut buf))
        .await
        .expect("a sender report")
        .unwrap();
    assert_eq!(n, 28);
    assert_eq!(buf[1], 200);
}

#[derive(Debug)]
struct FakeJpeg;

impl moli_runtime::media::SnapshotSource for FakeJpeg {
    fn snapshot(&self) -> BoxFuture<'_, anyhow::Result<moli_runtime::media::Image>> {
        Box::pin(async {
            Ok(moli_runtime::media::Image {
                content_type: "image/jpeg".into(),
                bytes: vec![0xFF, 0xD8, 1, 2, 3, 0xFF, 0xD9],
            })
        })
    }
}

/// One HTTP/1.1 exchange: (status, headers, body).
async fn http(at: std::net::SocketAddr, request: &str) -> (u16, String, Vec<u8>) {
    let mut stream = TcpStream::connect(at).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut all = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut all))
        .await
        .expect("an answer")
        .unwrap();
    let split = all.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8(all[..split].to_vec()).unwrap();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    (status, head, all[split + 4..].to_vec())
}

fn soap_request(at: std::net::SocketAddr, path: &str, header: &str, body: &str) -> String {
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope"><s:Header>{header}</s:Header><s:Body>{body}</s:Body></s:Envelope>"#
    );
    format!(
        "POST {path} HTTP/1.1\r\nHost: {at}\r\nContent-Type: application/soap+xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{xml}",
        xml.len()
    )
}

fn ws_token(password: &str) -> String {
    let created = jiff::Timestamp::now().to_string();
    let nonce = [7u8; 16];
    let mut input = nonce.to_vec();
    input.extend_from_slice(created.as_bytes());
    input.extend_from_slice(password.as_bytes());
    let digest = crate::connection::base64(
        ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, &input).as_ref(),
    );
    format!(
        r#"<Security xmlns="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-secext-1.0.xsd"><UsernameToken><Username>moli</Username><Password Type="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-username-token-profile-1.0#PasswordDigest">{digest}</Password><Nonce>{}</Nonce><Created xmlns="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd">{created}</Created></UsernameToken></Security>"#,
        crate::connection::base64(&nonce)
    )
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // one adoption, start to end
async fn a_recorder_adopts_the_camera_over_onvif() {
    let s = setup("onvif", Some(PASSWORD)).await.unwrap();
    let at = s.onvif.iter().find(|(p, _)| p == "door").unwrap().1;
    let at = std::net::SocketAddr::from(([127, 0, 0, 1], at.port()));

    // What a recorder asks first, without credentials: answered.
    let (code, head, body) = http(
        at,
        &soap_request(
            at,
            "/onvif/device_service",
            "",
            "<tds:GetSystemDateAndTime xmlns:tds=\"http://www.onvif.org/ver10/device/wsdl\"/>",
        ),
    )
    .await;
    assert_eq!(code, 200);
    assert!(head.to_ascii_lowercase().contains("application/soap+xml"));
    let body = String::from_utf8(body).unwrap();
    assert!(body.contains("<tt:UTCDateTime>"), "{body}");

    // With the right password: the device, its profiles and its stream.
    let token = ws_token(PASSWORD);
    let ask = |action: &str| {
        soap_request(
            at,
            "/onvif/media_service",
            &token,
            &format!(
                "<trt:{action} xmlns:trt=\"http://www.onvif.org/ver10/media/wsdl\"><trt:ProfileToken>main</trt:ProfileToken></trt:{action}>"
            ),
        )
    };
    let (code, _, body) = http(
        at,
        &soap_request(
            at,
            "/onvif/device_service",
            &token,
            "<GetDeviceInformation/>",
        ),
    )
    .await;
    assert_eq!(code, 200);
    let body = String::from_utf8(body).unwrap();
    assert!(body.contains("<tds:Manufacturer>Moli</tds:Manufacturer>"));
    assert!(body.contains("<tds:Model>Front door</tds:Model>"));
    let (code, _, body) = http(at, &ask("GetProfiles")).await;
    assert_eq!(code, 200);
    let body = String::from_utf8(body).unwrap();
    assert!(
        body.contains(r#"<trt:Profiles token="main""#)
            && body.contains(r#"<trt:Profiles token="sub""#)
    );
    assert!(body.contains("<tt:Encoding>H264</tt:Encoding>"));
    let (code, _, body) = http(at, &ask("GetVideoEncoderConfigurationOptions")).await;
    assert_eq!(code, 200);
    assert!(
        String::from_utf8(body)
            .unwrap()
            .contains("<tt:ResolutionsAvailable>")
    );
    let (code, _, body) = http(at, &ask("GetStreamUri")).await;
    assert_eq!(code, 200);
    let body = String::from_utf8(body).unwrap();
    assert!(
        body.contains(&format!(
            "<tt:Uri>rtsp://127.0.0.1:{}/door</tt:Uri>",
            s.port
        )),
        "{body}"
    );
    let (code, _, body) = http(at, &ask("GetSnapshotUri")).await;
    assert_eq!(code, 200);
    assert!(String::from_utf8(body).unwrap().contains(&format!(
        "http://127.0.0.1:{}/onvif/snapshot.jpg",
        at.port()
    )));
    let (code, _, body) = http(
        at,
        &soap_request(
            at,
            "/onvif/device_service",
            &token,
            "<GetCapabilities><Category>All</Category></GetCapabilities>",
        ),
    )
    .await;
    assert_eq!(code, 200);
    let body = String::from_utf8(body).unwrap();
    assert!(body.contains(&format!(
        "http://127.0.0.1:{}/onvif/media_service",
        at.port()
    )));

    // A wrong password is refused, an unknown action is a fault.
    let (code, _, body) = http(
        at,
        &soap_request(
            at,
            "/onvif/media_service",
            &ws_token("guess"),
            "<GetProfiles/>",
        ),
    )
    .await;
    assert_eq!(code, 400);
    assert!(
        String::from_utf8(body)
            .unwrap()
            .contains("ter:NotAuthorized")
    );
    let (code, _, body) = http(
        at,
        &soap_request(at, "/onvif/device_service", &token, "<SystemReboot/>"),
    )
    .await;
    assert_eq!(code, 400);
    assert!(
        String::from_utf8(body)
            .unwrap()
            .contains("ter:ActionNotSupported")
    );

    // The image: Digest asked, then given.
    let (code, head, _) = http(
        at,
        &format!("GET /onvif/snapshot.jpg HTTP/1.1\r\nHost: {at}\r\nConnection: close\r\n\r\n"),
    )
    .await;
    assert_eq!(code, 401);
    let challenge = head
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("www-authenticate"))
        .unwrap();
    assert!(challenge.contains("qop=\"auth\""));
    let nonce = challenge
        .split("nonce=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let ha1 = md5_hex(&format!("moli:Moli:{PASSWORD}"));
    let ha2 = md5_hex("GET:/onvif/snapshot.jpg");
    let response = md5_hex(&format!("{ha1}:{nonce}:00000001:abcdef:auth:{ha2}"));
    let (code, head, body) = http(at, &format!("GET /onvif/snapshot.jpg HTTP/1.1\r\nHost: {at}\r\nAuthorization: Digest username=\"moli\", realm=\"Moli\", nonce=\"{nonce}\", uri=\"/onvif/snapshot.jpg\", qop=auth, nc=00000001, cnonce=\"abcdef\", response=\"{response}\"\r\nConnection: close\r\n\r\n")).await;
    assert_eq!(code, 200, "{head}");
    assert!(head.to_ascii_lowercase().contains("image/jpeg"));
    assert_eq!(&body[..2], &[0xFF, 0xD8]);

    // The camera of a protected room: no description, no image.
    let nursery = s.onvif.iter().find(|(p, _)| p == "nursery").unwrap().1;
    let nursery = std::net::SocketAddr::from(([127, 0, 0, 1], nursery.port()));
    let (code, _, _) = http(
        nursery,
        &soap_request(nursery, "/onvif/media_service", &token, "<GetStreamUri/>"),
    )
    .await;
    assert_eq!(code, 400);
}

#[tokio::test]
async fn a_probe_on_the_network_finds_the_onvif_cameras() {
    let hub = Hub::new(HubOptions::default()).unwrap();
    let shared = Arc::new(crate::connection::Shared {
        hub,
        cameras: std::collections::HashMap::new(),
        user: "moli".into(),
        nonces: std::sync::Mutex::default(),
        port: 8554,
        seen: std::sync::Mutex::default(),
    });
    let camera = Arc::new(crate::onvif::Device {
        path: "door".into(),
        name: "Front door".into(),
        id: DeviceId::from("fake:door"),
        port: 8899,
        shared,
    });
    let responder = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let at = responder.local_addr().unwrap();
    let cancel = CancellationToken::new();
    let _guard = cancel.clone().drop_guard();
    crate::discovery::serve(responder, vec![Arc::clone(&camera)], None, cancel);
    let recorder = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let probe = r#"<?xml version="1.0" encoding="utf-8"?><Envelope xmlns:dn="http://www.onvif.org/ver10/network/wsdl" xmlns="http://www.w3.org/2003/05/soap-envelope"><Header><wsa:MessageID xmlns:wsa="http://schemas.xmlsoap.org/ws/2004/08/addressing">uuid:11111111-2222-3333-4444-555555555555</wsa:MessageID></Header><Body><Probe xmlns="http://schemas.xmlsoap.org/ws/2005/04/discovery"><Types>dn:NetworkVideoTransmitter</Types></Probe></Body></Envelope>"#;
    recorder.send_to(probe.as_bytes(), at).await.unwrap();
    let mut buf = vec![0u8; 8192];
    let (n, _) = tokio::time::timeout(Duration::from_secs(2), recorder.recv_from(&mut buf))
        .await
        .expect("an answer")
        .unwrap();
    let answer = std::str::from_utf8(&buf[..n]).unwrap();
    assert!(
        answer.contains("<a:RelatesTo>uuid:11111111-2222-3333-4444-555555555555</a:RelatesTo>")
    );
    assert!(
        answer.contains("http://127.0.0.1:8899/onvif/device_service"),
        "{answer}"
    );
    assert!(answer.contains(&format!("urn:uuid:{}", camera.uuid())));
    assert!(answer.contains("onvif://www.onvif.org/name/Front_door"));
}
