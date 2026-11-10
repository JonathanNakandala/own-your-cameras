//! The ONVIF HTTP adapter: one blocking thread per connection (capped, with a per-camera stack size),
//! bounded requests, read and write timeouts. Serves the SOAP services under `/onvif/` and the
//! authenticated `GET /snapshot.jpg`, waits for PullMessages, and stops a timed ContinuousMove.

use crate::auth::{self, NonceCache};
use crate::camera::OnvifCamera;
use crate::entropy::random;
use crate::events::PullPointBook;
use crate::service::{self, Context, Reply};
use crate::snapshot::{self, Plan, SnapshotCache};
use crate::soap::{self, Fault};
use camera_transport::Credentials;
use camera_transport::rtsp::proto::{challenge, check_digest};
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

const MAX_HEADER: usize = 8 * 1024;
const MAX_BODY: usize = 64 * 1024;
/// An idle keep-alive connection is closed after this.
const IDLE: Duration = Duration::from_secs(30);
/// A request that has started must arrive completely within this.
const REQUEST_TIME: Duration = Duration::from_secs(10);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
/// Digest nonces rotate this often; the previous one stays valid for one more period.
const NONCE_PERIOD: Duration = Duration::from_secs(300);
const RETRY_AFTER_S: u32 = 1;

/// Connection and worker-stack policy, chosen per camera.
#[derive(Clone, Copy, Debug)]
pub struct HttpLimits {
    pub connections: usize,
    pub thread_stack_bytes: Option<usize>,
}

impl HttpLimits {
    /// A recorder, Home Assistant (one held by its event long-poll) and a few snapshot fetches.
    pub const DEFAULT: Self = Self {
        connections: 8,
        thread_stack_bytes: Some(128 * 1024),
    };

    fn thread(self) -> std::thread::Builder {
        let builder = std::thread::Builder::new().name("onvif-http".into());
        match self.thread_stack_bytes {
            Some(bytes) => builder.stack_size(bytes),
            None => builder,
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The camera side of ONVIF events: report every input edge here (the same edges MQTT publishes).
pub struct Events {
    book: Mutex<PullPointBook>,
    changed: Condvar,
}

impl Events {
    fn new(inputs: &[&str]) -> Self {
        Self {
            book: Mutex::new(PullPointBook::new(inputs)),
            changed: Condvar::new(),
        }
    }

    /// Input `token` is now `state`.
    pub fn input(&self, token: &str, state: bool) {
        lock(&self.book).input(token, state, SystemTime::now());
        self.changed.notify_all();
    }
}

struct Snapshots {
    cache: Mutex<SnapshotCache>,
    done: Condvar,
}

/// Stops a ContinuousMove whose timeout ran out, unless a later move, Stop or preset recall came first.
struct PtzTimer {
    deadline: Mutex<Option<Instant>>,
    wake: Condvar,
}

struct DigestNonces {
    current: String,
    previous: String,
    since: Instant,
}

pub struct OnvifServer {
    camera: Arc<dyn OnvifCamera>,
    creds: Credentials,
    rtsp_port: u16,
    limits: HttpLimits,
    events: Arc<Events>,
    snapshots: Snapshots,
    tokens: Mutex<NonceCache>,
    digest: Mutex<DigestNonces>,
    ptz: PtzTimer,
}

impl OnvifServer {
    /// `creds` are the camera's RTSP credentials: ONVIF uses the same ones.
    pub fn new(
        camera: Arc<dyn OnvifCamera>,
        creds: Credentials,
        rtsp_port: u16,
        limits: HttpLimits,
    ) -> Arc<Self> {
        let nonce = crate::codec::hex(&random::<8>());
        Arc::new(Self {
            events: Arc::new(Events::new(camera.digital_inputs())),
            camera,
            creds,
            rtsp_port,
            limits,
            snapshots: Snapshots {
                cache: Mutex::default(),
                done: Condvar::new(),
            },
            tokens: Mutex::default(),
            digest: Mutex::new(DigestNonces {
                previous: nonce.clone(),
                current: nonce,
                since: Instant::now(),
            }),
            ptz: PtzTimer {
                deadline: Mutex::new(None),
                wake: Condvar::new(),
            },
        })
    }

    /// Where the camera reports input edges.
    pub fn events(&self) -> Arc<Events> {
        Arc::clone(&self.events)
    }

    /// Hardware JPEG encodes run so far (coalesced requests share one).
    pub fn snapshot_encodes(&self) -> u64 {
        lock(&self.snapshots.cache).encodes()
    }

    fn nonce(&self) -> String {
        let mut d = lock(&self.digest);
        if d.since.elapsed() >= NONCE_PERIOD {
            d.previous = std::mem::take(&mut d.current);
            d.current = crate::codec::hex(&random::<8>());
            d.since = Instant::now();
        }
        d.current.clone()
    }

    fn digest_ok(&self, header: Option<&str>, method: &str, target: &str) -> bool {
        let Some(header) = header else {
            return false;
        };
        self.nonce();
        let d = lock(&self.digest);
        [&d.current, &d.previous]
            .iter()
            .any(|n| check_digest(header, method, target, &self.creds, n))
    }
}

pub fn bind(port: u16) -> io::Result<TcpListener> {
    TcpListener::bind((Ipv4Addr::UNSPECIFIED, port))
}

struct Worker {
    socket: TcpStream,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.socket.shutdown(std::net::Shutdown::Both);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Runs until `stop` is set, then closes every connection and joins its worker.
pub fn serve(
    listener: &TcpListener,
    server: Arc<OnvifServer>,
    stop: &AtomicBool,
) -> io::Result<()> {
    listener.set_nonblocking(true)?;
    let http_port = listener.local_addr()?.port();
    let stop_flag = Arc::new(AtomicBool::new(false));
    let timer = if server.camera.ptz().is_some() {
        let (server, stop_flag) = (Arc::clone(&server), Arc::clone(&stop_flag));
        Some(
            std::thread::Builder::new()
                .name("onvif-ptz".into())
                .stack_size(64 * 1024)
                .spawn(move || ptz_timer(&server, &stop_flag))?,
        )
    } else {
        None
    };
    let mut workers: Vec<Worker> = Vec::new();
    let result = loop {
        if stop.load(Ordering::Relaxed) {
            break Ok(());
        }
        workers.retain(|w| w.thread.as_ref().is_some_and(|t| !t.is_finished()));
        match listener.accept() {
            Ok((sock, _)) => {
                if workers.len() >= server.limits.connections {
                    continue;
                }
                let Ok(socket) = sock.set_nonblocking(false).and_then(|()| sock.try_clone()) else {
                    continue;
                };
                let (srv, flag) = (Arc::clone(&server), Arc::clone(&stop_flag));
                // A failed spawn (no memory for a stack) drops this connection only.
                if let Ok(thread) = server.limits.thread().spawn(move || {
                    let _ = connection(sock, &srv, http_port, &flag);
                }) {
                    workers.push(Worker {
                        socket,
                        thread: Some(thread),
                    });
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
            }
            // Out of descriptors or buffers, or a client that gave up: wait and keep serving.
            Err(e)
                if matches!(
                    e.raw_os_error(),
                    Some(libc::EMFILE | libc::ENFILE | libc::ENOBUFS | libc::ENOMEM)
                ) || matches!(
                    e.kind(),
                    io::ErrorKind::ConnectionAborted
                        | io::ErrorKind::ConnectionReset
                        | io::ErrorKind::Interrupted
                ) =>
            {
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(e) => break Err(e),
        }
    };
    stop_flag.store(true, Ordering::Relaxed);
    server.events.changed.notify_all();
    server.ptz.wake.notify_all();
    workers.clear();
    if let Some(t) = timer {
        let _ = t.join();
    }
    result
}

fn ptz_timer(server: &OnvifServer, stop: &AtomicBool) {
    let mut deadline = lock(&server.ptz.deadline);
    while !stop.load(Ordering::Relaxed) {
        match *deadline {
            Some(at) if Instant::now() >= at => {
                *deadline = None;
                drop(deadline);
                if let Some(ptz) = server.camera.ptz() {
                    let _ = ptz.stop();
                }
                deadline = lock(&server.ptz.deadline);
            }
            Some(at) => {
                let wait = at
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(500));
                deadline = server
                    .ptz
                    .wake
                    .wait_timeout(deadline, wait)
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
            None => {
                deadline = server
                    .ptz
                    .wake
                    .wait_timeout(deadline, Duration::from_millis(500))
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
        }
    }
}

struct HttpRequest {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    keep_alive: bool,
}

impl HttpRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

enum ReadError {
    Closed,
    Bad(u16),
    Io(io::Error),
}

/// One read: an idle connection may wait [`IDLE`] for its next request, but once a request has started
/// it must be complete within [`REQUEST_TIME`] however slowly its bytes trickle in.
fn read_more(
    sock: &mut TcpStream,
    buf: &mut Vec<u8>,
    started: &mut Option<Instant>,
) -> Result<(), ReadError> {
    let timeout = match *started {
        None => IDLE,
        Some(at) => REQUEST_TIME
            .checked_sub(at.elapsed())
            .filter(|t| !t.is_zero())
            .ok_or(ReadError::Bad(408))?,
    };
    sock.set_read_timeout(Some(timeout))
        .map_err(ReadError::Io)?;
    let mut chunk = [0u8; 2048];
    match sock.read(&mut chunk) {
        Ok(0) => Err(ReadError::Closed),
        Ok(n) => {
            started.get_or_insert_with(Instant::now);
            buf.extend_from_slice(&chunk[..n]);
            Ok(())
        }
        Err(e)
            if started.is_some()
                && matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
        {
            Err(ReadError::Bad(408))
        }
        Err(e) => Err(ReadError::Io(e)),
    }
}

fn read_request(sock: &mut TcpStream, buf: &mut Vec<u8>) -> Result<HttpRequest, ReadError> {
    let mut started = (!buf.is_empty()).then(Instant::now);
    let head_end = loop {
        if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break at;
        }
        if buf.len() > MAX_HEADER {
            return Err(ReadError::Bad(431));
        }
        read_more(sock, buf, &mut started)?;
    };
    let head = std::str::from_utf8(&buf[..head_end]).map_err(|_| ReadError::Bad(400))?;
    let mut lines = head.split("\r\n");
    let mut start = lines.next().unwrap_or_default().split(' ');
    let (method, target, version) = (
        start.next().unwrap_or_default().to_string(),
        start.next().unwrap_or_default().to_string(),
        start.next().unwrap_or_default().to_string(),
    );
    if method.is_empty() || !target.starts_with('/') || !version.starts_with("HTTP/1.") {
        return Err(ReadError::Bad(400));
    }
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    let find = |name: &str| {
        headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    };
    if find("Transfer-Encoding").is_some() {
        return Err(ReadError::Bad(411));
    }
    let length: usize = match find("Content-Length") {
        Some(v) => v.parse().map_err(|_| ReadError::Bad(400))?,
        None => 0,
    };
    if length > MAX_BODY {
        return Err(ReadError::Bad(413));
    }
    let keep_alive = match find("Connection") {
        Some(c) if c.eq_ignore_ascii_case("close") => false,
        Some(c) if c.eq_ignore_ascii_case("keep-alive") => true,
        _ => version == "HTTP/1.1",
    };
    let body_start = head_end + 4;
    while buf.len() < body_start + length {
        read_more(sock, buf, &mut started)?;
    }
    let body = buf[body_start..body_start + length].to_vec();
    buf.drain(..body_start + length);
    Ok(HttpRequest {
        method,
        target,
        headers,
        body,
        keep_alive,
    })
}

struct HttpResponse {
    status: u16,
    content_type: &'static str,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
}

impl HttpResponse {
    fn soap(status: u16, xml: String) -> Self {
        Self {
            status,
            content_type: "application/soap+xml; charset=utf-8",
            headers: Vec::new(),
            body: xml.into_bytes(),
        }
    }

    fn fault(fault: &Fault) -> Self {
        Self::soap(fault.status(), fault.render())
    }

    fn plain(status: u16) -> Self {
        Self {
            status,
            content_type: "text/plain",
            headers: Vec::new(),
            body: Vec::new(),
        }
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        408 => "Request Timeout",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        411 => "Length Required",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        503 => "Service Unavailable",
        _ => "Internal Server Error",
    }
}

fn write_response(sock: &mut TcpStream, r: &HttpResponse, keep_alive: bool) -> io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: {}\r\n",
        r.status,
        reason(r.status),
        r.content_type,
        r.body.len(),
        if keep_alive { "keep-alive" } else { "close" }
    );
    for (k, v) in &r.headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    sock.write_all(head.as_bytes())?;
    sock.write_all(&r.body)
}

fn connection(
    mut sock: TcpStream,
    server: &OnvifServer,
    http_port: u16,
    stop: &AtomicBool,
) -> io::Result<()> {
    sock.set_write_timeout(Some(WRITE_TIMEOUT))?;
    sock.set_nodelay(true)?;
    let local = sock.local_addr()?;
    let mut buf = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        let req = match read_request(&mut sock, &mut buf) {
            Ok(r) => r,
            Err(ReadError::Bad(status)) => {
                return write_response(&mut sock, &HttpResponse::plain(status), false);
            }
            Err(ReadError::Closed) => return Ok(()),
            Err(ReadError::Io(e)) => return Err(e),
        };
        let response = route(&req, server, local, http_port, stop);
        let keep = req.keep_alive && !stop.load(Ordering::Relaxed);
        write_response(&mut sock, &response, keep)?;
        if !keep {
            return Ok(());
        }
    }
    Ok(())
}

fn host_of(local: SocketAddr) -> String {
    match local.ip() {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => match ip.to_ipv4_mapped() {
            Some(v4) => v4.to_string(),
            None => format!("[{ip}]"),
        },
    }
}

fn route(
    req: &HttpRequest,
    server: &OnvifServer,
    local: SocketAddr,
    http_port: u16,
    stop: &AtomicBool,
) -> HttpResponse {
    let path = req.target.split('?').next().unwrap_or_default();
    if path == "/snapshot.jpg" {
        if req.method != "GET" {
            return HttpResponse::plain(405);
        }
        if !server.digest_ok(req.header("Authorization"), &req.method, &req.target) {
            return unauthorized(server, HttpResponse::plain(401));
        }
        return snapshot_response(server);
    }
    if !path.starts_with("/onvif/") {
        return HttpResponse::plain(404);
    }
    if req.method != "POST" {
        return HttpResponse::plain(405);
    }
    let Ok(soap_req) = soap::parse(&req.body) else {
        return HttpResponse::fault(&Fault::sender(&["ter:WellFormed"], "Not a SOAP envelope"));
    };
    if !service::public(&soap_req) {
        match &soap_req.token {
            Some(token) => {
                let verdict = auth::verify(
                    token,
                    &server.creds,
                    SystemTime::now(),
                    &mut lock(&server.tokens),
                );
                if verdict.is_err() {
                    return HttpResponse::fault(&Fault::not_authorized());
                }
            }
            None if server.digest_ok(req.header("Authorization"), &req.method, &req.target) => {}
            None => {
                return unauthorized(server, HttpResponse::fault(&Fault::not_authorized()));
            }
        }
    }
    let host = host_of(local);
    let http_host = format!("{host}:{http_port}");
    let subscription = path
        .strip_prefix("/onvif/subscription/")
        .and_then(|id| id.parse().ok());
    let now = SystemTime::now();
    let ctx = Context {
        now,
        http_host: &http_host,
        host: &host,
        rtsp_port: server.rtsp_port,
        http_port,
        camera: server.camera.as_ref(),
        user: &server.creds.user,
        subscription,
    };
    // Only the Events services touch the subscription book; every other call (PTZ moves, preset writes
    // to flash) runs without it, so input edges are never held up behind the camera.
    let reply = if matches!(soap_req.ns.as_str(), soap::TEV | soap::WSNT) {
        service::handle(&soap_req, &ctx, &mut lock(&server.events.book))
    } else {
        service::handle(&soap_req, &ctx, &mut PullPointBook::default())
    };
    match reply {
        Reply::Ok(body) => HttpResponse::soap(200, soap::envelope("", &body)),
        Reply::Fault(f) => HttpResponse::fault(&f),
        Reply::Moving { body, stop_after } => {
            *lock(&server.ptz.deadline) = Some(Instant::now() + stop_after);
            server.ptz.wake.notify_all();
            HttpResponse::soap(200, soap::envelope("", &body))
        }
        Reply::Stopped(body) => {
            *lock(&server.ptz.deadline) = None;
            server.ptz.wake.notify_all();
            HttpResponse::soap(200, soap::envelope("", &body))
        }
        Reply::Pull { id, wait, limit } => pull(server, id, wait, limit, stop),
    }
}

fn unauthorized(server: &OnvifServer, mut r: HttpResponse) -> HttpResponse {
    r.status = 401;
    r.headers
        .push(("WWW-Authenticate", challenge(&server.nonce())));
    r
}

fn pull(
    server: &OnvifServer,
    id: u32,
    wait: Duration,
    limit: usize,
    stop: &AtomicBool,
) -> HttpResponse {
    let until = Instant::now() + wait;
    let mut book = lock(&server.events.book);
    while book.exists(id) && !book.has_messages(id) && !stop.load(Ordering::Relaxed) {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        book = server
            .events
            .changed
            .wait_timeout(book, left.min(Duration::from_millis(500)))
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
    let now = SystemTime::now();
    match book.pull(id, now, limit) {
        Ok((messages, expires)) => HttpResponse::soap(
            200,
            soap::envelope("", &service::pull_response(now, expires, &messages)),
        ),
        Err(_) => HttpResponse::fault(&Fault::sender(
            &["ter:InvalidArgVal", "ter:NoSubscription"],
            "The subscription ended",
        )),
    }
}

fn busy() -> HttpResponse {
    let mut r = HttpResponse::plain(503);
    r.headers.push(("Retry-After", RETRY_AFTER_S.to_string()));
    r
}

fn jpeg(image: &[u8]) -> HttpResponse {
    HttpResponse {
        status: 200,
        content_type: "image/jpeg",
        headers: vec![("Cache-Control", "no-store".into())],
        body: image.to_vec(),
    }
}

fn snapshot_response(server: &OnvifServer) -> HttpResponse {
    let deadline = Instant::now() + snapshot::DEADLINE;
    let s = &server.snapshots;
    let mut cache = lock(&s.cache);
    match cache.plan(Instant::now()) {
        Plan::Serve(image) => jpeg(&image),
        Plan::Encode => {
            drop(cache);
            let result = server.camera.snapshot(deadline);
            let mut cache = lock(&s.cache);
            cache.finished(Instant::now(), result.as_ref().ok().cloned());
            s.done.notify_all();
            match result {
                Ok(image) => jpeg(&image),
                Err(_) => busy(),
            }
        }
        Plan::Wait => loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return busy();
            }
            cache = s
                .done
                .wait_timeout(cache, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
            if let Some(image) = cache.ready(Instant::now()) {
                return jpeg(&image);
            }
            if !cache.busy() {
                // The encode we waited for failed: no stale image, no second encode.
                return busy();
            }
        },
    }
}

#[cfg(test)]
mod tests;
