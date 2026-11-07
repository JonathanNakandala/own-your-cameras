//! Contract tests: the real HTTP adapter on an isolated loopback port, a hand-written client and the
//! fake camera. No hardware, no network beyond 127.0.0.1.
use super::*;
use crate::entropy::random;
use crate::fake::FakeCamera;
use crate::soap::{TDS, TEV, TPTZ, TRT};
use camera_transport::rtsp::md5::md5_hex;
use std::io::{BufRead, BufReader};

const USER: &str = "admin";
const PASSWORD: &str = "test-password";

struct Fixture {
    addr: SocketAddr,
    server: Arc<OnvifServer>,
    camera: Arc<FakeCamera>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<io::Result<()>>>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(w) = self.worker.take() {
            w.join().unwrap().unwrap();
        }
    }
}

fn start(camera: FakeCamera) -> Fixture {
    let camera = Arc::new(camera);
    let server = OnvifServer::new(
        Arc::clone(&camera) as Arc<dyn OnvifCamera>,
        Credentials {
            user: USER.into(),
            password: PASSWORD.into(),
        },
        554,
        HttpLimits::DEFAULT,
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let (s, srv) = (Arc::clone(&stop), Arc::clone(&server));
    let worker = Some(std::thread::spawn(move || serve(&listener, srv, &s)));
    Fixture {
        addr,
        server,
        camera,
        stop,
        worker,
    }
}

struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    fn text(&self) -> String {
        String::from_utf8(self.body.clone()).unwrap()
    }
}

struct Client(BufReader<TcpStream>);

impl Client {
    fn connect(addr: SocketAddr) -> Self {
        let s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
        Self(BufReader::new(s))
    }

    fn send(&mut self, raw: &[u8]) -> Reply {
        self.0.get_mut().write_all(raw).unwrap();
        let mut line = String::new();
        self.0.read_line(&mut line).unwrap();
        let status = line.split(' ').nth(1).unwrap().parse().unwrap();
        let mut headers = Vec::new();
        loop {
            line.clear();
            self.0.read_line(&mut line).unwrap();
            let l = line.trim_end();
            if l.is_empty() {
                break;
            }
            let (k, v) = l.split_once(':').unwrap();
            headers.push((k.to_string(), v.trim().to_string()));
        }
        let len: usize = headers
            .iter()
            .find(|(k, _)| k == "Content-Length")
            .map_or(0, |(_, v)| v.parse().unwrap());
        let mut body = vec![0; len];
        self.0.read_exact(&mut body).unwrap();
        Reply {
            status,
            headers,
            body,
        }
    }

    fn post(&mut self, path: &str, xml: &str, extra: &str) -> Reply {
        let raw = format!(
            "POST {path} HTTP/1.1\r\nHost: CAMERA_HOST\r\nContent-Type: application/soap+xml\r\nContent-Length: {}\r\n{extra}\r\n{xml}",
            xml.len()
        );
        self.send(raw.as_bytes())
    }

    fn get(&mut self, path: &str, extra: &str) -> Reply {
        self.send(format!("GET {path} HTTP/1.1\r\nHost: CAMERA_HOST\r\n{extra}\r\n").as_bytes())
    }
}

fn security(password: &str, nonce: &[u8; 16]) -> String {
    let created = crate::time::format(SystemTime::now());
    let digest = crate::codec::base64_encode(&crate::auth::digest(nonce, &created, password));
    format!(
        r#"<s:Header><Security xmlns="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-secext-1.0.xsd"><UsernameToken><Username>{USER}</Username><Password Type="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-username-token-profile-1.0#PasswordDigest">{digest}</Password><Nonce>{}</Nonce><Created xmlns="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd">{created}</Created></UsernameToken></Security></s:Header>"#,
        crate::codec::base64_encode(nonce)
    )
}

fn envelope(header: &str, ns: &str, action: &str, body: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope" xmlns:tt="http://www.onvif.org/ver10/schema">{header}<s:Body><{action} xmlns="{ns}">{body}</{action}></s:Body></s:Envelope>"#
    )
}

fn signed(ns: &str, action: &str, body: &str) -> String {
    envelope(&security(PASSWORD, &random::<16>()), ns, action, body)
}

/// `Authorization: Digest …` for `method target` from a 401's challenge (RFC 2617 without qop, as the
/// camera offers it).
fn digest_header(challenge: &str, method: &str, target: &str, password: &str) -> String {
    let nonce = challenge
        .split("nonce=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let ha1 = md5_hex(format!("{USER}:cam:{password}").as_bytes());
    let ha2 = md5_hex(format!("{method}:{target}").as_bytes());
    let response = md5_hex(format!("{ha1}:{nonce}:{ha2}").as_bytes());
    format!(
        "Authorization: Digest username=\"{USER}\", realm=\"cam\", nonce=\"{nonce}\", uri=\"{target}\", response=\"{response}\"\r\n"
    )
}

#[test]
fn discovery_calls_are_open_and_everything_else_needs_credentials() {
    let f = start(FakeCamera::new(false, &[]));
    let mut c = Client::connect(f.addr);
    let open = c.post(
        "/onvif/device_service",
        &envelope("", TDS, "GetSystemDateAndTime", ""),
        "",
    );
    assert_eq!(open.status, 200);
    assert!(
        open.text()
            .contains("<tt:DateTimeType>NTP</tt:DateTimeType>")
    );
    assert_eq!(open.header("Connection"), Some("keep-alive"));
    let closed = c.post(
        "/onvif/media_service",
        &envelope("", TRT, "GetProfiles", ""),
        "",
    );
    assert_eq!(closed.status, 401);
    assert!(
        closed
            .header("WWW-Authenticate")
            .unwrap()
            .starts_with("Digest realm=\"cam\"")
    );
    assert!(closed.text().contains("ter:NotAuthorized"));
    let ok = c.post("/onvif/media_service", &signed(TRT, "GetProfiles", ""), "");
    assert_eq!(ok.status, 200);
    assert!(ok.text().contains("token=\"main\""));
    // Surveillance Station's unauthenticated CreateProfile probe gets the real answer, not 401.
    let probe = c.post(
        "/onvif/media_service",
        &envelope("", TRT, "CreateProfile", "<Name>SynoTestProfile</Name>"),
        "",
    );
    assert_eq!(probe.status, 500);
    assert!(probe.text().contains("ter:MaxNVTProfiles"));
}

#[test]
fn wrong_stale_and_replayed_tokens_are_refused() {
    let f = start(FakeCamera::new(false, &[]));
    let mut c = Client::connect(f.addr);
    let wrong = envelope(
        &security("nope", &random::<16>()),
        TDS,
        "GetDeviceInformation",
        "",
    );
    let r = c.post("/onvif/device_service", &wrong, "");
    assert_eq!(r.status, 400);
    assert!(r.text().contains("ter:NotAuthorized"));
    let once = signed(TDS, "GetDeviceInformation", "");
    assert_eq!(c.post("/onvif/device_service", &once, "").status, 200);
    assert_eq!(
        c.post("/onvif/device_service", &once, "").status,
        400,
        "replay"
    );
    let stale = envelope(
        &security(PASSWORD, &random::<16>()),
        TDS,
        "GetDeviceInformation",
        "",
    )
    .replace(&crate::time::format(SystemTime::now())[..4], "2001");
    assert_eq!(c.post("/onvif/device_service", &stale, "").status, 400);
}

#[test]
fn http_digest_authenticates_soap_and_the_snapshot() {
    let f = start(FakeCamera::new(false, &[]));
    let mut c = Client::connect(f.addr);
    let challenge = c.get("/snapshot.jpg", "");
    assert_eq!(challenge.status, 401);
    let www = challenge.header("WWW-Authenticate").unwrap().to_string();
    let bad = c.get(
        "/snapshot.jpg",
        &digest_header(&www, "GET", "/snapshot.jpg", "nope"),
    );
    assert_eq!(bad.status, 401);
    let good = c.get(
        "/snapshot.jpg",
        &digest_header(&www, "GET", "/snapshot.jpg", PASSWORD),
    );
    assert_eq!(good.status, 200);
    assert_eq!(good.header("Content-Type"), Some("image/jpeg"));
    assert_eq!(good.body, [0xff, 0xd8, 0xff, 0xd9]);
    let soap = c.post(
        "/onvif/device_service",
        &envelope("", TDS, "GetDeviceInformation", ""),
        &digest_header(&www, "POST", "/onvif/device_service", PASSWORD),
    );
    assert_eq!(soap.status, 200);
    assert!(
        soap.text()
            .contains("<tds:SerialNumber>020000000001</tds:SerialNumber>")
    );
}

#[test]
fn parallel_and_back_to_back_snapshot_requests_share_one_encode() {
    let mut cam = FakeCamera::new(false, &[]);
    cam.encode_time = Duration::from_millis(300);
    let f = start(cam);
    let www = Client::connect(f.addr)
        .get("/snapshot.jpg", "")
        .header("WWW-Authenticate")
        .unwrap()
        .to_string();
    let addr = f.addr;
    let clients: Vec<_> = (0..8)
        .map(|_| {
            let www = www.clone();
            std::thread::spawn(move || {
                let mut c = Client::connect(addr);
                c.get(
                    "/snapshot.jpg",
                    &digest_header(&www, "GET", "/snapshot.jpg", PASSWORD),
                )
                .status
            })
        })
        .collect();
    let statuses: Vec<u16> = clients.into_iter().map(|t| t.join().unwrap()).collect();
    assert!(statuses.iter().all(|&s| s == 200), "{statuses:?}");
    // Two more within the freshness window share the same image too.
    for _ in 0..2 {
        let r = Client::connect(f.addr).get(
            "/snapshot.jpg",
            &digest_header(&www, "GET", "/snapshot.jpg", PASSWORD),
        );
        assert_eq!(r.status, 200);
    }
    assert_eq!(f.camera.encodes.load(Ordering::SeqCst), 1);
    assert_eq!(f.server.snapshot_encodes(), 1);
}

#[test]
fn a_slow_encoder_gets_a_503_with_retry_after() {
    let mut cam = FakeCamera::new(false, &[]);
    cam.encode_time = snapshot::DEADLINE + Duration::from_millis(200);
    let f = start(cam);
    let mut c = Client::connect(f.addr);
    let www = c
        .get("/snapshot.jpg", "")
        .header("WWW-Authenticate")
        .unwrap()
        .to_string();
    let r = c.get(
        "/snapshot.jpg",
        &digest_header(&www, "GET", "/snapshot.jpg", PASSWORD),
    );
    assert_eq!(r.status, 503);
    assert_eq!(r.header("Retry-After"), Some("1"));
}

#[test]
fn a_pull_waits_for_the_next_input_edge() {
    let f = start(FakeCamera::new(false, &["call_button", "door"]));
    let mut c = Client::connect(f.addr);
    let created = c.post(
        "/onvif/events_service",
        &signed(
            TEV,
            "CreatePullPointSubscription",
            "<InitialTerminationTime>PT10M</InitialTerminationTime>",
        ),
        "",
    );
    assert_eq!(created.status, 200);
    let text = created.text();
    let address = text
        .split("<wsa:Address>")
        .nth(1)
        .unwrap()
        .split('<')
        .next()
        .unwrap();
    let path = &address[address.find("/onvif/").unwrap()..];
    let pull = signed(
        TEV,
        "PullMessages",
        "<Timeout>PT10S</Timeout><MessageLimit>10</MessageLimit>",
    );
    let first = c.post(path, &pull, "");
    assert_eq!(
        first
            .text()
            .matches("PropertyOperation=\"Initialized\"")
            .count(),
        2
    );
    let events = f.server.events();
    let pusher = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        events.input("call_button", true);
    });
    let started = Instant::now();
    let next = c.post(
        path,
        &signed(
            TEV,
            "PullMessages",
            "<Timeout>PT10S</Timeout><MessageLimit>10</MessageLimit>",
        ),
        "",
    );
    pusher.join().unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "answered at the edge, not the timeout"
    );
    let body = next.text();
    assert!(body.contains("PropertyOperation=\"Changed\""));
    assert!(
        body.contains("Value=\"call_button\"")
            && body.contains("Name=\"LogicalState\" Value=\"true\"")
    );
    let quiet = c.post(
        path,
        &signed(
            TEV,
            "PullMessages",
            "<Timeout>PT1S</Timeout><MessageLimit>10</MessageLimit>",
        ),
        "",
    );
    assert_eq!(quiet.status, 200);
    assert!(!quiet.text().contains("NotificationMessage"));
}

#[test]
fn a_timed_continuous_move_stops_by_itself() {
    let f = start(FakeCamera::new(true, &[]));
    let mut c = Client::connect(f.addr);
    let r = c.post(
        "/onvif/ptz_service",
        &signed(
            TPTZ,
            "ContinuousMove",
            r#"<ProfileToken>main</ProfileToken><Velocity><tt:PanTilt x="0.5" y="0"/></Velocity><Timeout>PT0.3S</Timeout>"#,
        ),
        "",
    );
    assert_eq!(r.status, 200);
    let ptz = f.camera.ptz.as_ref().unwrap();
    let started = Instant::now();
    while !ptz.calls().contains(&"stop".to_string()) {
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "never stopped: {:?}",
            ptz.calls()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(started.elapsed() >= Duration::from_millis(200));
}

#[test]
fn malformed_and_misrouted_requests_get_plain_http_errors() {
    let f = start(FakeCamera::new(false, &[]));
    assert_eq!(Client::connect(f.addr).get("/nothing", "").status, 404);
    assert_eq!(
        Client::connect(f.addr)
            .get("/onvif/device_service", "")
            .status,
        405
    );
    let r = Client::connect(f.addr).post("/onvif/device_service", "not xml", "");
    assert_eq!(r.status, 400);
    assert!(r.text().contains("ter:WellFormed"));
    let big = Client::connect(f.addr)
        .send(b"POST /onvif/device_service HTTP/1.1\r\nContent-Length: 999999\r\n\r\n");
    assert_eq!(big.status, 413);
    let chunked = Client::connect(f.addr)
        .send(b"POST /onvif/device_service HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n");
    assert_eq!(chunked.status, 411);
}
