//! End to end over loopback: a real listener, a hand-written RTSP client, frames from a [`Hub`].
use super::*;
use crate::rtsp::md5::md5_hex;
use std::io::{BufRead, BufReader};

const SPS: &[u8] = &[0, 0, 0, 1, 0x67, 0x4d, 0, 0x1f, 0xac];
const PPS: &[u8] = &[0, 0, 0, 1, 0x68, 0xee, 0x3c];

fn idr(n: usize) -> Vec<u8> {
    let mut v = [SPS, PPS, &[0, 0, 0, 1, 0x65]].concat();
    v.extend(std::iter::repeat_n(0x42, n));
    v
}

struct Fixture {
    addr: SocketAddr,
    hub: Arc<Hub>,
    audio: Arc<AudioHub>,
    pcmu: Arc<AudioHub>,
    talk: Arc<Talkback>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<io::Result<()>>>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap().unwrap();
        }
    }
}

fn start(creds: Option<Credentials>) -> Fixture {
    start_mode(creds, true)
}

fn start_mode(creds: Option<Credentials>, audio_enabled: bool) -> Fixture {
    let hub = Hub::new();
    hub.publish(true, 0, idr(10));
    let audio = AudioHub::new();
    let pcmu = AudioHub::with_format(crate::microphone::Format::Pcmu8k);
    let talk = crate::rtsp::talk::fake::talkback();
    let cfg = Arc::new(Config {
        streams: ["sub", "main"]
            .map(|p| {
                StreamPath::new(p, Arc::clone(&hub))
                    .audio(audio_enabled.then(|| Arc::clone(&audio)))
            })
            .into_iter()
            .chain([StreamPath::new(MAIN_PCMU_PATH, Arc::clone(&hub))
                .audio(audio_enabled.then(|| Arc::clone(&pcmu)))
                .backchannel(Backchannel::Pcmu)])
            .collect(),
        creds,
        talk: audio_enabled.then(|| Arc::clone(&talk)),
    });
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let s = Arc::clone(&stop);
    let worker = Some(std::thread::spawn(move || serve(&listener, cfg, &s)));
    Fixture {
        addr,
        hub,
        stop,
        audio,
        pcmu,
        talk,
        worker,
    }
}

struct Client {
    r: BufReader<TcpStream>,
    cseq: u32,
}

struct Reply {
    code: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Reply {
    fn h(&self, k: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.as_str())
    }
}

impl Client {
    fn connect(addr: SocketAddr) -> Self {
        let s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        Self {
            r: BufReader::new(s),
            cseq: 0,
        }
    }

    fn call(&mut self, method: &str, url: &str, extra: &[(&str, &str)]) -> Reply {
        self.cseq += 1;
        let mut s = format!("{method} {url} RTSP/1.0\r\nCSeq: {}\r\n", self.cseq);
        for (k, v) in extra {
            s.push_str(&format!("{k}: {v}\r\n"));
        }
        s.push_str("\r\n");
        self.r.get_mut().write_all(s.as_bytes()).unwrap();
        let mut line = String::new();
        self.r.read_line(&mut line).unwrap();
        let code = line.split(' ').nth(1).unwrap().parse().unwrap();
        let mut headers = Vec::new();
        loop {
            line.clear();
            self.r.read_line(&mut line).unwrap();
            let l = line.trim_end();
            if l.is_empty() {
                break;
            }
            let (k, v) = l.split_once(':').unwrap();
            headers.push((k.to_string(), v.trim().to_string()));
        }
        let len = headers
            .iter()
            .find(|(k, _)| k == "Content-Length")
            .map_or(0, |(_, v)| v.parse().unwrap());
        let mut body = vec![0; len];
        self.r.read_exact(&mut body).unwrap();
        Reply {
            code,
            headers,
            body: String::from_utf8(body).unwrap(),
        }
    }

    /// One interleaved frame: (channel, payload).
    fn interleaved(&mut self) -> (u8, Vec<u8>) {
        let mut h = [0u8; 4];
        self.r.read_exact(&mut h).unwrap();
        assert_eq!(h[0], b'$');
        let mut p = vec![0; usize::from(u16::from_be_bytes([h[2], h[3]]))];
        self.r.read_exact(&mut p).unwrap();
        (h[1], p)
    }
}

const URL: &str = "rtsp://127.0.0.1/sub";

#[test]
fn stopping_the_server_closes_idle_connections_and_releases_the_speaker() {
    let mut f = start(None);
    let mut c = Client::connect(f.addr);
    c.call(
        "DESCRIBE",
        URL,
        &[("Require", "www.onvif.org/ver20/backchannel")],
    );
    assert_eq!(
        c.call(
            "SETUP",
            &format!("{URL}/trackID=2"),
            &[("Transport", "RTP/AVP/TCP;interleaved=4-5")]
        )
        .code,
        200
    );
    assert_eq!(f.talk.talkers(), 1);
    f.stop.store(true, Ordering::Relaxed);
    f.worker.take().unwrap().join().unwrap().unwrap();
    assert_eq!(
        f.talk.talkers(),
        0,
        "serve returned with a live connection worker"
    );
    assert_eq!(c.r.read(&mut [0]).unwrap(), 0);
}

#[test]
fn repeated_play_and_teardown_release_viewers_even_when_the_encoder_is_idle() {
    let f = start(None);
    let mut c = Client::connect(f.addr);
    assert_eq!(
        c.call(
            "SETUP",
            URL,
            &[("Transport", "RTP/AVP/TCP;interleaved=0-1")]
        )
        .code,
        200
    );
    assert_eq!(
        c.call(
            "SETUP",
            &format!("{URL}/trackID=1"),
            &[("Transport", "RTP/AVP/TCP;interleaved=2-3")]
        )
        .code,
        200
    );
    for _ in 0..3 {
        assert_eq!(c.call("PLAY", URL, &[]).code, 200);
        assert_eq!(
            f.hub.viewers(),
            1,
            "previous PLAY sender is still subscribed"
        );
        assert_eq!(
            f.audio.viewers(),
            1,
            "previous microphone sender is still subscribed"
        );
    }
    assert_eq!(c.call("TEARDOWN", URL, &[]).code, 200);
    assert_eq!(
        f.hub.viewers(),
        0,
        "TEARDOWN returned before sender cleanup"
    );
    assert_eq!(
        f.audio.viewers(),
        0,
        "TEARDOWN retained microphone subscriptions"
    );
}

#[test]
fn a_viewer_that_stops_reading_cannot_hold_a_sender_forever() {
    let f = start(None);
    let mut c = Client::connect(f.addr);
    assert_eq!(
        c.call(
            "SETUP",
            URL,
            &[("Transport", "RTP/AVP/TCP;interleaved=0-1")]
        )
        .code,
        200
    );
    assert_eq!(c.call("PLAY", URL, &[]).code, 200);
    // Sustained valid-size frames fill the TCP window while the client never reads RTP.
    // A single giant frame is rejected by the AU ceiling before reaching the sender.
    let deadline = std::time::Instant::now() + Duration::from_secs(6);
    while f.hub.viewers() != 0 && std::time::Instant::now() < deadline {
        f.hub.publish(true, 0, idr(512 * 1024));
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(f.hub.viewers(), 0, "non-reading client retained its sender");
}

#[test]
fn describe_setup_play_streams_rtp_over_tcp_interleaved() {
    let f = start(None);
    let mut c = Client::connect(f.addr);
    assert_eq!(c.call("OPTIONS", URL, &[]).code, 200);
    let d = c.call("DESCRIBE", URL, &[("Accept", "application/sdp")]);
    assert_eq!(d.code, 200);
    assert!(d.body.contains("H264/90000"));
    assert!(d.body.contains("sprop-parameter-sets="));
    let s = c.call(
        "SETUP",
        &format!("{URL}/trackID=0"),
        &[("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1")],
    );
    assert_eq!(s.code, 200);
    assert!(s.h("Transport").unwrap().contains("interleaved=0-1"));
    let session = s
        .h("Session")
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    assert_eq!(c.call("PLAY", URL, &[("Session", &session)]).code, 200);

    f.hub.publish(true, 1_000_000, idr(3000)); // forces FU-A
    let mut nal_types = Vec::new();
    let mut marker_seen = false;
    while !marker_seen {
        let (ch, p) = c.interleaved();
        assert_eq!(ch, 0);
        assert_eq!(p[0], 0x80);
        assert_eq!(&p[4..8], &90_000u32.to_be_bytes());
        nal_types.push(p[12] & 0x1f);
        marker_seen = p[1] & 0x80 != 0;
    }
    assert_eq!(&nal_types[..2], &[7, 8]);
    assert!(nal_types.contains(&28));
    assert_eq!(c.call("TEARDOWN", URL, &[("Session", &session)]).code, 200);
}

#[test]
fn unknown_path_is_404_and_bad_transport_461() {
    let f = start(None);
    let mut c = Client::connect(f.addr);
    assert_eq!(c.call("DESCRIBE", "rtsp://h/nope", &[]).code, 404);
    let s = c.call("SETUP", URL, &[("Transport", "RTP/AVP;unicast")]);
    assert_eq!(s.code, 461);
    assert_eq!(c.call("PLAY", URL, &[]).code, 455);
}

#[test]
fn digest_is_required_and_a_correct_response_passes() {
    let f = start(Some(Credentials {
        user: "admin".into(),
        password: "pw".into(),
    }));
    let mut c = Client::connect(f.addr);
    assert_eq!(c.call("OPTIONS", URL, &[]).code, 200);
    let r = c.call("DESCRIBE", URL, &[]);
    assert_eq!(r.code, 401);
    let ch = r.h("WWW-Authenticate").unwrap();
    let nonce = ch
        .split("nonce=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let ha1 = md5_hex(b"admin:cam:pw");
    let ha2 = md5_hex(format!("DESCRIBE:{URL}").as_bytes());
    let resp = md5_hex(format!("{ha1}:{nonce}:{ha2}").as_bytes());
    let auth = format!(
        "Digest username=\"admin\", realm=\"cam\", nonce=\"{nonce}\", uri=\"{URL}\", response=\"{resp}\""
    );
    assert_eq!(
        c.call("DESCRIBE", URL, &[("Authorization", &auth)]).code,
        200
    );
    let bad = auth.replace(&resp, &"0".repeat(32));
    assert_eq!(
        c.call("DESCRIBE", URL, &[("Authorization", &bad)]).code,
        401
    );
}

#[test]
fn udp_transport_delivers_rtp_to_the_client_port() {
    let f = start(None);
    let rx = UdpSocket::bind("127.0.0.1:0").unwrap();
    rx.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let port = rx.local_addr().unwrap().port();
    let mut c = Client::connect(f.addr);
    let s = c.call(
        "SETUP",
        URL,
        &[(
            "Transport",
            &format!("RTP/AVP;unicast;client_port={port}-{}", port + 1),
        )],
    );
    assert_eq!(s.code, 200);
    assert!(s.h("Transport").unwrap().contains("server_port="));
    c.call("PLAY", URL, &[]);
    f.hub.publish(true, 0, idr(20));
    let mut b = [0u8; 2000];
    let n = rx.recv(&mut b).unwrap();
    assert_eq!(b[0], 0x80);
    assert_eq!(b[12] & 0x1f, 7);
    assert!(n > 12);
}

#[test]
fn dropping_the_connection_unsubscribes_the_viewer() {
    let f = start(None);
    {
        let mut c = Client::connect(f.addr);
        c.call(
            "SETUP",
            URL,
            &[("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1")],
        );
        c.call("PLAY", URL, &[]);
        assert_eq!(f.hub.viewers(), 1);
    }
    for _ in 0..50 {
        f.hub.publish(true, 0, idr(5));
        if f.hub.viewers() == 0 {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("viewer still subscribed");
}

#[test]
fn describe_offers_opus_and_only_negotiated_opus_backchannel_on_both_paths() {
    let f = start(None);
    let mut c = Client::connect(f.addr);
    for path in ["sub", "main"] {
        let url = format!("rtsp://127.0.0.1/{path}");
        let plain = c.call("DESCRIBE", &url, &[]);
        assert_eq!(plain.code, 200);
        assert!(plain.body.contains(
            "a=rtpmap:111 opus/48000/2\r\na=fmtp:111 sprop-stereo=0\r\na=control:trackID=1"
        ));
        assert!(!plain.body.contains("trackID=2"));
        assert!(!plain.body.contains("MPEG4") && !plain.body.contains("PCMU"));
        let talk = c.call(
            "DESCRIBE",
            &url,
            &[("Require", "www.onvif.org/ver20/backchannel")],
        );
        assert!(talk.body.contains(
            "m=audio 0 RTP/AVP 111\r\na=rtpmap:111 opus/48000/2\r\na=fmtp:111 sprop-stereo=0\r\na=control:trackID=2\r\na=sendonly"
        ));
    }
}

#[test]
fn audio_setup_does_not_replace_video_and_both_arrive_after_play() {
    let f = start(None);
    let mut c = Client::connect(f.addr);
    for (track, channels) in [(0, "0-1"), (1, "2-3")] {
        let r = c.call(
            "SETUP",
            &format!("{URL}/trackID={track}"),
            &[(
                "Transport",
                &format!("RTP/AVP/TCP;unicast;interleaved={channels}"),
            )],
        );
        assert_eq!(r.code, 200);
    }
    assert_eq!(c.call("PLAY", URL, &[]).code, 200);
    f.hub.publish(true, 1_000_000, idr(10));
    f.audio.publish(1_000_000, vec![1, 2, 3]);
    let mut seen = [false; 2];
    for _ in 0..4 {
        let (channel, data) = c.interleaved();
        match channel {
            0 => seen[0] = true,
            2 => {
                assert_eq!(data[1], 0x80 | 111);
                assert_eq!(&data[12..], &[1, 2, 3]);
                seen[1] = true;
            }
            _ => panic!("unexpected channel {channel}"),
        }
        if seen == [true, true] {
            return;
        }
    }
    panic!("missing a track: {seen:?}");
}

#[test]
fn negotiated_backchannel_decodes_only_rtp_after_play() {
    let f = start(None);
    let mut c = Client::connect(f.addr);
    let track = format!("{URL}/trackID=2");
    let transport = [(
        "Transport",
        "RTP/AVP/TCP;unicast;interleaved=4-5;mode=record",
    )];
    assert_ne!(c.call("SETUP", &track, &transport).code, 200);
    c.call(
        "DESCRIBE",
        URL,
        &[("Require", "www.onvif.org/ver20/backchannel")],
    );
    assert_eq!(c.call("SETUP", &track, &transport).code, 200);
    let packet = [0x80, 111, 0, 1, 0, 0, 0, 0, 1, 2, 3, 4, 0x80];
    let send = |c: &mut Client, channel: u8| {
        c.r.get_mut()
            .write_all(&[b'$', channel, 0, packet.len() as u8])
            .unwrap();
        c.r.get_mut().write_all(&packet).unwrap();
    };
    send(&mut c, 4);
    assert_eq!(c.call("GET_PARAMETER", URL, &[]).code, 200);
    assert!(f.talk.next_pcm(std::time::Instant::now()).is_none());
    assert_eq!(c.call("PLAY", URL, &[]).code, 200);
    send(&mut c, 5); // RTCP channel: must not reach the speaker.
    for _ in 0..5 {
        send(&mut c, 4); // five 20 ms packets fill the 100 ms prebuffer
    }
    assert_eq!(c.call("GET_PARAMETER", URL, &[]).code, 200);
    // The fake decoder's level 2048, through the start gain: raised and held under the -1 dBFS ceiling.
    let pcm = f.talk.next_pcm(std::time::Instant::now()).unwrap();
    assert_eq!(pcm.len(), 320);
    assert!(
        pcm.iter().all(|&s| (6_000..=29_196).contains(&s)),
        "{pcm:?}"
    );
    assert_eq!(c.call("TEARDOWN", URL, &[]).code, 200);
}

#[test]
fn a_fifth_talker_is_rejected_until_one_tears_down() {
    let f = start(None);
    let transport = [(
        "Transport",
        "RTP/AVP/TCP;unicast;interleaved=4-5;mode=record",
    )];
    let track = format!("{URL}/trackID=2");
    let mut clients: Vec<Client> = (0..5).map(|_| Client::connect(f.addr)).collect();
    for c in &mut clients {
        c.call(
            "DESCRIBE",
            URL,
            &[("Require", "www.onvif.org/ver20/backchannel")],
        );
    }
    for c in &mut clients[..4] {
        assert_eq!(c.call("SETUP", &track, &transport).code, 200);
    }
    assert_eq!(clients[4].call("SETUP", &track, &transport).code, 453);
    assert_eq!(f.talk.talkers(), 4);
    assert_eq!(clients[0].call("TEARDOWN", URL, &[]).code, 200);
    assert_eq!(clients[4].call("SETUP", &track, &transport).code, 200);
}

#[test]
fn track_channels_cannot_overlap_and_unknown_tracks_are_rejected() {
    let f = start(None);
    let mut c = Client::connect(f.addr);
    let tcp = [("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1")];
    assert_eq!(c.call("SETUP", &format!("{URL}/trackID=0"), &tcp).code, 200);
    assert_eq!(c.call("SETUP", &format!("{URL}/trackID=1"), &tcp).code, 461);
    assert_eq!(c.call("SETUP", &format!("{URL}/trackID=8"), &tcp).code, 404);
}

#[test]
fn playing_another_path_does_not_activate_a_configured_backchannel() {
    let f = start(None);
    let mut c = Client::connect(f.addr);
    c.call(
        "DESCRIBE",
        URL,
        &[("Require", "www.onvif.org/ver20/backchannel")],
    );
    assert_eq!(
        c.call(
            "SETUP",
            &format!("{URL}/trackID=2"),
            &[("Transport", "RTP/AVP/TCP;interleaved=4-5;mode=record")]
        )
        .code,
        200
    );
    let main = "rtsp://127.0.0.1/main";
    c.call(
        "SETUP",
        &format!("{main}/trackID=1"),
        &[("Transport", "RTP/AVP/TCP;interleaved=2-3")],
    );
    c.call("PLAY", main, &[]);
    c.r.get_mut()
        .write_all(
            &[
                b'$', 4, 0, 13, 0x80, 111, 0, 1, 0, 0, 0, 0, 1, 2, 3, 4, 0x80,
            ]
            .repeat(2),
        )
        .unwrap();
    c.call("GET_PARAMETER", main, &[]);
    assert!(f.talk.next_pcm(std::time::Instant::now()).is_none());
}

#[test]
fn video_only_config_offers_no_audio_and_rejects_audio_setup() {
    let f = start_mode(None, false);
    let mut c = Client::connect(f.addr);
    let d = c.call(
        "DESCRIBE",
        URL,
        &[("Require", "www.onvif.org/ver20/backchannel")],
    );
    assert_eq!(d.code, 200);
    assert!(!d.body.contains("m=audio"));
    for track in [1, 2] {
        assert_eq!(
            c.call(
                "SETUP",
                &format!("{URL}/trackID={track}"),
                &[("Transport", "RTP/AVP/TCP;interleaved=2-3")]
            )
            .code,
            404
        );
    }
}

#[test]
fn opus_is_delivered_over_udp_with_its_dynamic_payload_type() {
    let f = start(None);
    let rx = UdpSocket::bind("127.0.0.1:0").unwrap();
    rx.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
    let port = rx.local_addr().unwrap().port();
    let mut c = Client::connect(f.addr);
    assert_eq!(
        c.call(
            "SETUP",
            &format!("{URL}/trackID=1"),
            &[(
                "Transport",
                &format!("RTP/AVP;unicast;client_port={port}-{}", port + 1)
            )]
        )
        .code,
        200
    );
    c.call("PLAY", URL, &[]);
    f.audio.publish(1_000_000, vec![1, 2, 3]);
    let mut b = [0u8; 1500];
    let n = rx.recv(&mut b).unwrap();
    assert_eq!(b[1], 0x80 | 111);
    assert_eq!(u32::from_be_bytes(b[4..8].try_into().unwrap()), 48_000);
    assert_eq!(&b[12..n], &[1, 2, 3]);
}

#[test]
fn unauthenticated_rtp_and_setup_never_claim_the_speaker() {
    let f = start(Some(Credentials {
        user: "admin".into(),
        password: "pw".into(),
    }));
    let mut c = Client::connect(f.addr);
    assert_eq!(
        c.call(
            "DESCRIBE",
            URL,
            &[("Require", "www.onvif.org/ver20/backchannel")]
        )
        .code,
        401
    );
    assert_eq!(
        c.call(
            "SETUP",
            &format!("{URL}/trackID=2"),
            &[("Transport", "RTP/AVP/TCP;interleaved=4-5")]
        )
        .code,
        401
    );
    c.r.get_mut()
        .write_all(
            &[
                b'$', 4, 0, 13, 0x80, 111, 0, 1, 0, 0, 0, 0, 1, 2, 3, 4, 0x80,
            ]
            .repeat(2),
        )
        .unwrap();
    c.call("OPTIONS", URL, &[]); // request handled after the RTP
    assert!(f.talk.next_pcm(std::time::Instant::now()).is_none());
    assert_eq!(f.talk.talkers(), 0);
}

#[test]
fn a_digest_signed_for_another_uri_is_rejected() {
    let f = start(Some(Credentials {
        user: "admin".into(),
        password: "pw".into(),
    }));
    let mut c = Client::connect(f.addr);
    let r = c.call("DESCRIBE", URL, &[]);
    let nonce = r
        .h("WWW-Authenticate")
        .unwrap()
        .split("nonce=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_string();
    let sign = |uri: &str| {
        let ha1 = md5_hex(b"admin:cam:pw");
        let ha2 = md5_hex(format!("DESCRIBE:{uri}").as_bytes());
        let resp = md5_hex(format!("{ha1}:{nonce}:{ha2}").as_bytes());
        format!(
            "Digest username=\"admin\", realm=\"cam\", nonce=\"{nonce}\", uri=\"{uri}\", response=\"{resp}\""
        )
    };
    let other = sign("rtsp://127.0.0.1/main");
    assert_eq!(
        c.call("DESCRIBE", URL, &[("Authorization", &other)]).code,
        401
    );
    let own = sign(URL);
    assert_eq!(
        c.call("DESCRIBE", URL, &[("Authorization", &own)]).code,
        200
    );
}

#[test]
fn play_response_precedes_the_first_rtp_while_frames_are_being_published() {
    let f = start(None);
    let running = Arc::new(AtomicBool::new(true));
    let publisher = {
        let (hub, running) = (Arc::clone(&f.hub), Arc::clone(&running));
        std::thread::spawn(move || {
            let mut pts = 0;
            while running.load(Ordering::Relaxed) {
                pts += 33_000;
                hub.publish(true, pts, idr(200));
                std::thread::sleep(Duration::from_millis(1));
            }
        })
    };
    for _ in 0..40 {
        let mut c = Client::connect(f.addr);
        assert_eq!(c.call("DESCRIBE", URL, &[]).code, 200);
        let s = c.call(
            "SETUP",
            &format!("{URL}/trackID=0"),
            &[("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1")],
        );
        let session = s
            .h("Session")
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();
        // `call` panics if the first bytes are an interleaved frame rather than a status line.
        assert_eq!(c.call("PLAY", URL, &[("Session", &session)]).code, 200);
        let (ch, _) = c.interleaved();
        assert_eq!(ch, 0);
    }
    running.store(false, Ordering::Relaxed);
    publisher.join().unwrap();
}

#[test]
fn a_failed_sender_closes_the_connection_and_releases_the_speaker() {
    let f = start(None);
    let mut c = Client::connect(f.addr);
    c.call(
        "DESCRIBE",
        URL,
        &[("Require", "www.onvif.org/ver20/backchannel")],
    );
    for (track, channels) in [(0, "0-1"), (2, "4-5")] {
        assert_eq!(
            c.call(
                "SETUP",
                &format!("{URL}/trackID={track}"),
                &[("Transport", &format!("RTP/AVP/TCP;interleaved={channels}"))]
            )
            .code,
            200
        );
    }
    assert_eq!(c.call("PLAY", URL, &[]).code, 200);
    assert_eq!(f.talk.talkers(), 1);
    let deadline = std::time::Instant::now() + Duration::from_secs(6);
    while std::time::Instant::now() < deadline {
        f.hub.publish(true, 0, idr(512 * 1024));
        if f.talk.talkers() == 0 {
            assert_eq!(f.hub.viewers(), 0);
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("failed sender retained the connection's exclusive speaker lease");
}

#[test]
fn setup_cannot_reassign_channels_while_the_previous_sender_is_playing() {
    let f = start(None);
    let mut c = Client::connect(f.addr);
    assert_eq!(
        c.call(
            "SETUP",
            URL,
            &[("Transport", "RTP/AVP/TCP;interleaved=0-1")]
        )
        .code,
        200
    );
    assert_eq!(c.call("PLAY", URL, &[]).code, 200);
    assert_eq!(
        c.call(
            "SETUP",
            URL,
            &[("Transport", "RTP/AVP/TCP;interleaved=2-3")]
        )
        .code,
        455
    );
    assert_eq!(
        c.call(
            "SETUP",
            &format!("{URL}/trackID=1"),
            &[("Transport", "RTP/AVP/TCP;interleaved=0-1")]
        )
        .code,
        455
    );
    assert_eq!(c.call("TEARDOWN", URL, &[]).code, 200);
    assert_eq!(f.hub.viewers(), 0);
}

#[test]
fn the_onvif_path_carries_pcmu_both_ways_while_the_camera_paths_keep_opus() {
    let f = start(None);
    let mut c = Client::connect(f.addr);
    let url = format!("rtsp://127.0.0.1/{MAIN_PCMU_PATH}");
    let d = c.call(
        "DESCRIBE",
        &url,
        &[("Require", "www.onvif.org/ver20/backchannel")],
    );
    assert_eq!(d.code, 200);
    assert!(
        d.body
            .contains("a=rtpmap:0 PCMU/8000/1\r\na=control:trackID=1\r\na=recvonly")
    );
    assert!(d.body.contains(
        "m=audio 0 RTP/AVP 0\r\na=rtpmap:0 PCMU/8000/1\r\na=control:trackID=2\r\na=sendonly"
    ));
    assert!(!d.body.contains("opus"));
    for (track, channels) in [(1, "2-3"), (2, "4-5")] {
        let r = c.call(
            "SETUP",
            &format!("{url}/trackID={track}"),
            &[(
                "Transport",
                &format!("RTP/AVP/TCP;unicast;interleaved={channels}"),
            )],
        );
        assert_eq!(r.code, 200);
    }
    assert_eq!(c.call("PLAY", &url, &[]).code, 200);
    f.pcmu.publish(1_000_000, vec![0xff; 160]);
    let (channel, rtp) = c.interleaved();
    assert_eq!(channel, 2);
    assert_eq!(rtp[1] & 127, 0, "PCMU payload type");
    assert_eq!(rtp.len(), 12 + 160);
    assert_eq!(f.audio.viewers(), 0, "the Opus hub has no listener");
    let mut packet = vec![0x80, 0, 0, 1, 0, 0, 0, 0, 1, 2, 3, 4];
    packet.extend([0x9f; 160]); // a loud constant level
    for _ in 0..6 {
        let stream = c.r.get_mut();
        stream.write_all(&[b'$', 4, 0, packet.len() as u8]).unwrap();
        stream.write_all(&packet).unwrap();
    }
    assert_eq!(c.call("GET_PARAMETER", &url, &[]).code, 200);
    let pcm = f.talk.next_pcm(std::time::Instant::now()).unwrap();
    assert_eq!(pcm.len(), 320);
}
