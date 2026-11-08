//! The direct call link: a small TCP server the indoor panel keeps open, so ringing, answering and
//! opening work without Home Assistant. Newline-delimited JSON ([`crate::call_wire`]); the client
//! proves the RTSP account with `md5(user:nonce:password)` against a fresh nonce.
//!
//! Only transport here: commands go to the runtime's one call owner through a channel, and the
//! runtime pushes every snapshot and acknowledgment back. One thread per client (at most
//! [`LinkConfig::max_clients`]) reads with a short timeout and writes its own queue in between, so
//! a stuck client never blocks the runtime. Every authenticated client is the device `panel`.
use crate::call::{CallId, Op, Refusal};
use crate::call_wire::{self, ClientFrame, MAX_FRAME};
use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct LinkConfig {
    pub user: String,
    pub password: String,
    pub max_clients: usize,
    /// A client this long without a frame is dropped (its lease is lost).
    pub silence: Duration,
    /// Time allowed to answer the challenge.
    pub auth_within: Duration,
}

impl LinkConfig {
    pub fn new(user: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            user: user.into(),
            password: password.into(),
            max_clients: 2,
            silence: Duration::from_secs(15),
            auth_within: Duration::from_secs(5),
        }
    }
}

/// What the link tells the runtime.
#[derive(Debug, PartialEq, Eq)]
pub enum LinkEvent {
    /// A client authenticated; the runtime should send it the current snapshot (it already got the
    /// last published one).
    Connected,
    Command {
        conn: u64,
        req: u32,
        call: Option<CallId>,
        op: Op,
    },
    /// The last authenticated client went away or fell silent: the panel's lease is lost.
    Lost,
}

struct Client {
    out: Sender<String>,
    authed: bool,
}

#[derive(Default)]
struct Shared {
    clients: HashMap<u64, Client>,
    last_state: Option<String>,
}

/// The runtime's handle on the link.
#[derive(Clone)]
pub struct Link {
    shared: Arc<Mutex<Shared>>,
}

impl Link {
    /// Every authenticated client gets this snapshot now; new ones get it on connect.
    pub fn publish(&self, snapshot_json: &str) {
        let frame = call_wire::state_frame(snapshot_json);
        let mut s = self.lock();
        s.last_state = Some(frame.clone());
        for c in s.clients.values().filter(|c| c.authed) {
            let _ = c.out.send(frame.clone());
        }
    }
    pub fn ack(&self, conn: u64, req: u32, result: Result<(), Refusal>) {
        if let Some(c) = self.lock().clients.get(&conn) {
            let _ = c.out.send(call_wire::ack(req, result));
        }
    }
    /// An authenticated panel is connected.
    pub fn connected(&self) -> bool {
        self.lock().clients.values().any(|c| c.authed)
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Serves `listener` until `stop`; returns the handle and the event queue.
pub fn serve(
    listener: TcpListener,
    cfg: LinkConfig,
    stop: Arc<AtomicBool>,
    log: impl Fn(String) + Send + Sync + 'static,
) -> (Link, Receiver<LinkEvent>) {
    let (tx, rx) = mpsc::channel();
    let link = Link {
        shared: Arc::new(Mutex::new(Shared::default())),
    };
    let log = Arc::new(log);
    let next = Arc::new(AtomicU32::new(1));
    let server = link.clone();
    let _ = listener.set_nonblocking(true);
    let spawn = thread::Builder::new()
        .name("call-link".into())
        .spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let stream = match listener.accept() {
                    Ok((s, _)) => s,
                    Err(e) if e.kind() == ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(100));
                        continue;
                    }
                    Err(e) => {
                        log(format!("call-link: accept: {e}"));
                        thread::sleep(Duration::from_millis(500));
                        continue;
                    }
                };
                let conn = u64::from(next.fetch_add(1, Ordering::Relaxed));
                let (out_tx, out_rx) = mpsc::channel();
                {
                    let mut s = server.lock();
                    // Authenticated and pending connections are capped separately, so hosts that
                    // never authenticate cannot keep the panel out.
                    let authed = s.clients.values().filter(|c| c.authed).count();
                    if authed >= cfg.max_clients || s.clients.len() - authed >= cfg.max_clients {
                        log("call-link: client refused, too many".into());
                        continue;
                    }
                    s.clients.insert(
                        conn,
                        Client {
                            out: out_tx,
                            authed: false,
                        },
                    );
                }
                let (client_link, cfg, tx, log, stop) = (
                    server.clone(),
                    cfg.clone(),
                    tx.clone(),
                    log.clone(),
                    stop.clone(),
                );
                let spawned = thread::Builder::new()
                    .name("call-link-c".into())
                    .spawn(move || {
                        let authed =
                            client(stream, conn, &client_link, &cfg, &tx, out_rx, &stop, &*log);
                        let remaining = {
                            let mut s = client_link.lock();
                            s.clients.remove(&conn);
                            s.clients.values().any(|c| c.authed)
                        };
                        if authed && !remaining {
                            let _ = tx.send(LinkEvent::Lost);
                        }
                    });
                if spawned.is_err() {
                    link_drop(&server, conn);
                }
            }
        });
    if let Err(e) = spawn {
        eprintln!("call-link: not started: {e}");
    }
    (link, rx)
}

fn link_drop(link: &Link, conn: u64) {
    link.lock().clients.remove(&conn);
}

fn nonce() -> String {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let mut b = [0u8; 8];
    let ok = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b));
    if ok.is_err() {
        // Not secret, only fresh: the clock and a counter when urandom is unreadable.
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64)
            ^ (u64::from(COUNTER.fetch_add(1, Ordering::Relaxed)) << 48);
        b = t.to_le_bytes();
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// One client until it leaves, falls silent or misbehaves; returns whether it had authenticated.
#[allow(clippy::too_many_arguments)]
fn client(
    mut stream: TcpStream,
    conn: u64,
    link: &Link,
    cfg: &LinkConfig,
    tx: &Sender<LinkEvent>,
    out: Receiver<String>,
    stop: &AtomicBool,
    log: &(dyn Fn(String) + Send + Sync),
) -> bool {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let challenge = nonce();
    let write = |stream: &mut TcpStream, line: &str| {
        stream
            .write_all(line.as_bytes())
            .and_then(|_| stream.write_all(b"\n"))
    };
    if write(&mut stream, &call_wire::hello(&challenge)).is_err() {
        return false;
    }
    let started = Instant::now();
    let mut last_frame = Instant::now();
    let mut authed = false;
    let mut buf: Vec<u8> = Vec::with_capacity(MAX_FRAME);
    let mut chunk = [0u8; 256];
    while !stop.load(Ordering::Relaxed) {
        while let Ok(line) = out.try_recv() {
            if write(&mut stream, &line).is_err() {
                return authed;
            }
        }
        if !authed && started.elapsed() >= cfg.auth_within {
            log("call-link: no authentication, closed".into());
            return false;
        }
        if last_frame.elapsed() >= cfg.silence {
            log("call-link: client silent, closed".into());
            return authed;
        }
        let n = match stream.read(&mut chunk) {
            Ok(0) => return authed,
            Ok(n) => n,
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => continue,
            Err(_) => return authed,
        };
        buf.extend_from_slice(&chunk[..n]);
        while let Some(end) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=end).collect();
            let Ok(text) = std::str::from_utf8(&line[..line.len() - 1]) else {
                log("call-link: not UTF-8, closed".into());
                return authed;
            };
            let text = text.trim_end_matches('\r');
            if text.is_empty() {
                continue;
            }
            last_frame = Instant::now();
            match (authed, call_wire::parse_client_frame(text)) {
                (false, Ok(ClientFrame::Auth { user, response })) => {
                    let want = call_wire::auth_response(&cfg.user, &challenge, &cfg.password);
                    if user != cfg.user || !call_wire::same_response(&response, &want) {
                        log("call-link: authentication failed, closed".into());
                        return false;
                    }
                    authed = true;
                    let state = {
                        let mut s = link.lock();
                        if let Some(c) = s.clients.get_mut(&conn) {
                            c.authed = true;
                        }
                        s.last_state.clone()
                    };
                    if let Some(state) = state
                        && write(&mut stream, &state).is_err()
                    {
                        return true;
                    }
                    let _ = tx.send(LinkEvent::Connected);
                    log("call-link: panel connected".into());
                }
                (true, Ok(ClientFrame::Cmd { req, call, op })) => {
                    let _ = tx.send(LinkEvent::Command {
                        conn,
                        req,
                        call,
                        op,
                    });
                }
                (true, Ok(ClientFrame::Ping)) => {
                    if write(&mut stream, &call_wire::pong()).is_err() {
                        return true;
                    }
                }
                (_, r) => {
                    log(format!("call-link: unexpected frame ({r:?}), closed"));
                    return authed;
                }
            }
        }
        if buf.len() > MAX_FRAME {
            log("call-link: frame too long, closed".into());
            return authed;
        }
    }
    authed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};

    struct Peer {
        r: BufReader<TcpStream>,
        w: TcpStream,
    }

    impl Peer {
        fn connect(port: u16) -> Self {
            let w = TcpStream::connect(("127.0.0.1", port)).unwrap();
            w.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            Self {
                r: BufReader::new(w.try_clone().unwrap()),
                w,
            }
        }
        fn line(&mut self) -> serde_json::Value {
            let mut s = String::new();
            self.r.read_line(&mut s).unwrap();
            serde_json::from_str(&s).unwrap_or_else(|e| panic!("{e}: {s:?}"))
        }
        fn eof(&mut self) -> bool {
            let mut s = String::new();
            matches!(self.r.read_line(&mut s), Ok(0) | Err(_))
        }
        fn send(&mut self, text: &str) {
            self.w.write_all(text.as_bytes()).unwrap();
            self.w.write_all(b"\n").unwrap();
        }
        fn auth(&mut self, password: &str) {
            let hello = self.line();
            assert_eq!(hello["t"], "hello");
            let nonce = hello["nonce"].as_str().unwrap();
            let r = call_wire::auth_response("viewer", nonce, password);
            self.send(&format!(
                r#"{{"t":"auth","user":"viewer","response":"{r}"}}"#
            ));
        }
    }

    fn start(silence_ms: u64) -> (Link, Receiver<LinkEvent>, u16, Arc<AtomicBool>) {
        let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let mut cfg = LinkConfig::new("viewer", "secret");
        cfg.silence = Duration::from_millis(silence_ms);
        cfg.auth_within = Duration::from_millis(1_000);
        let (link, rx) = serve(l, cfg, stop.clone(), |_| {});
        (link, rx, port, stop)
    }

    fn event(rx: &Receiver<LinkEvent>) -> LinkEvent {
        rx.recv_timeout(Duration::from_secs(3)).expect("event")
    }

    #[test]
    fn auth_then_state_command_ack_and_ping() {
        let (link, rx, port, stop) = start(5_000);
        link.publish(r#"{"seq":1,"phase":"idle"}"#);
        let mut p = Peer::connect(port);
        p.auth("secret");
        let state = p.line();
        assert_eq!(
            (state["t"].as_str(), state["seq"].as_i64()),
            (Some("state"), Some(1))
        );
        assert_eq!(event(&rx), LinkEvent::Connected);
        assert!(link.connected());
        p.send(r#"{"t":"cmd","req":4,"call_id":"a3f9c2e1","op":"answer","mic":true}"#);
        let LinkEvent::Command {
            conn,
            req,
            call,
            op,
        } = event(&rx)
        else {
            panic!()
        };
        assert_eq!(
            (req, call, op),
            (4, Some([0xa3, 0xf9, 0xc2, 0xe1]), Op::Answer { mic: true })
        );
        link.ack(conn, req, Err(Refusal::Stale));
        let ack = p.line();
        assert_eq!(
            (ack["req"].as_i64(), ack["refusal"].as_str()),
            (Some(4), Some("stale"))
        );
        link.publish(r#"{"seq":2,"phase":"ringing"}"#);
        assert_eq!(p.line()["seq"], 2);
        p.send(r#"{"t":"ping"}"#);
        assert_eq!(p.line()["t"], "pong");
        drop(p);
        assert_eq!(event(&rx), LinkEvent::Lost);
        stop.store(true, Ordering::Relaxed);
    }

    #[test]
    fn a_wrong_password_or_garbage_closes_without_events() {
        let (link, rx, port, stop) = start(5_000);
        let mut p = Peer::connect(port);
        p.auth("wrong");
        assert!(p.eof());
        let mut p = Peer::connect(port);
        p.line();
        p.send(r#"{"t":"cmd","req":1,"op":"open"}"#);
        assert!(p.eof(), "commands before auth close the link");
        let mut p = Peer::connect(port);
        p.line();
        p.send("not json");
        assert!(p.eof());
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
        assert!(!link.connected());
        stop.store(true, Ordering::Relaxed);
    }

    #[test]
    fn pending_connections_cannot_keep_the_panel_out() {
        let (_link, rx, port, stop) = start(5_000);
        let mut squat = [Peer::connect(port), Peer::connect(port)];
        for s in &mut squat {
            s.line();
        }
        let mut third = Peer::connect(port);
        assert!(third.eof(), "a third pending connection is refused");
        drop(squat);
        std::thread::sleep(Duration::from_millis(200));
        let mut p = Peer::connect(port);
        p.auth("secret");
        assert_eq!(event(&rx), LinkEvent::Connected);
        stop.store(true, Ordering::Relaxed);
    }

    #[test]
    fn a_silent_client_loses_the_lease_and_no_auth_is_closed() {
        let (_link, rx, port, stop) = start(300);
        let mut p = Peer::connect(port);
        p.auth("secret");
        assert_eq!(event(&rx), LinkEvent::Connected);
        assert_eq!(event(&rx), LinkEvent::Lost);
        assert!(p.eof());
        let mut q = Peer::connect(port);
        q.line();
        assert!(q.eof(), "closed after auth_within");
        stop.store(true, Ordering::Relaxed);
    }

    #[test]
    fn the_lease_survives_while_another_panel_connection_remains_and_clients_are_capped() {
        let (_link, rx, port, stop) = start(5_000);
        let mut a = Peer::connect(port);
        a.auth("secret");
        assert_eq!(event(&rx), LinkEvent::Connected);
        let mut b = Peer::connect(port);
        b.auth("secret");
        assert_eq!(event(&rx), LinkEvent::Connected);
        let mut c = Peer::connect(port);
        assert!(c.eof(), "a third client is refused");
        drop(a);
        assert!(
            rx.recv_timeout(Duration::from_millis(400)).is_err(),
            "b still holds the lease"
        );
        drop(b);
        assert_eq!(event(&rx), LinkEvent::Lost);
        stop.store(true, Ordering::Relaxed);
    }
}
