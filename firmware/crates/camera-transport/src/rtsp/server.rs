//! The RTSP listener: one thread per connection, plus one sender thread per playing connection. Methods:
//! OPTIONS, DESCRIBE, SETUP, PLAY, TEARDOWN, GET_PARAMETER (keep-alive). RTP goes interleaved on the RTSP
//! TCP connection or over UDP to the client's ports.

use std::collections::HashMap;
use std::io::{self, IoSlice, Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use super::audio::{AudioHub, AudioViewer};
use super::hub::{Hub, Viewer};
use super::proto::{
    Credentials, Parsed, Request, Response, Transport, challenge, check_digest, parse_request,
    parse_transport, sdp,
};
use super::rtp::{Packetizer, timestamp};
use super::talk::{TalkLease, Talkback};

const SESSION_TIMEOUT_S: u64 = 60;
/// Connection and worker-stack policy is independent for each listener.
#[derive(Clone, Copy, Debug)]
pub struct ServerLimits {
    connections: usize,
    thread_stack_bytes: Option<usize>,
}
impl ServerLimits {
    pub const REOLINK: Self = Self {
        connections: 8,
        thread_stack_bytes: None,
    };
    /// go2rtc, a direct listener and up to four direct talkers; talk-only connections hold no video queue.
    pub const MI360: Self = Self {
        connections: 6,
        thread_stack_bytes: Some(96 * 1024),
    };
    fn thread(self) -> std::thread::Builder {
        let builder = std::thread::Builder::new();
        match self.thread_stack_bytes {
            Some(bytes) => builder.stack_size(bytes),
            None => builder,
        }
    }
}
impl Default for ServerLimits {
    fn default() -> Self {
        Self::REOLINK
    }
}

/// RTP payload bytes: 1500 MTU less IP/TCP/RTP headers, leaving room for Wi-Fi and the interleave frame.
const MAX_PAYLOAD: usize = 1400;
const DESCRIBE_WAIT: Duration = Duration::from_secs(5);
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);

pub struct Config {
    /// Path (`Preview_01_main`) → its stream.
    pub streams: Vec<(String, Arc<Hub>)>,
    /// `None` serves without authentication.
    pub creds: Option<Credentials>,
    /// One 48 kHz mono Opus capture shared by main and sub (track 1); absent until hardware is ready.
    pub audio: Option<Arc<AudioHub>>,
    /// Opus backchannel (track 2): up to four talkers mixed into mono samples at the camera's speaker rate.
    pub talk: Option<Arc<Talkback>>,
}

struct ConnectionWorker {
    socket: TcpStream,
    thread: Option<JoinHandle<()>>,
}

impl Drop for ConnectionWorker {
    fn drop(&mut self) {
        // Interrupt both the reader and any sender sharing this socket before joining.
        let _ = self.socket.shutdown(Shutdown::Both);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Runs until `stop` is set, then closes all connections and joins their workers.
pub fn bind(port: u16) -> io::Result<TcpListener> {
    bind_with_limits(port, ServerLimits::REOLINK)
}

pub fn bind_with_limits(port: u16, limits: ServerLimits) -> io::Result<TcpListener> {
    let listener = TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, port))?;
    super::socket_limits::listener(&listener, limits.connections as i32)?;
    Ok(listener)
}

pub fn serve(listener: &TcpListener, cfg: Arc<Config>, stop: &AtomicBool) -> io::Result<()> {
    serve_with(listener, cfg, stop, None)
}

/// As [`serve`], reporting `Rtsp` progress after every completed accept/supervision iteration, with or
/// without viewers. A listener that stops iterating (or errors) stops beating.
pub fn serve_with(
    listener: &TcpListener,
    cfg: Arc<Config>,
    stop: &AtomicBool,
    beats: Option<&dyn Fn()>,
) -> io::Result<()> {
    serve_with_limits(listener, cfg, stop, beats, ServerLimits::REOLINK)
}

pub fn serve_with_limits(
    listener: &TcpListener,
    cfg: Arc<Config>,
    stop: &AtomicBool,
    beats: Option<&dyn Fn()>,
    limits: ServerLimits,
) -> io::Result<()> {
    super::socket_limits::listener(listener, limits.connections as i32)?;
    listener.set_nonblocking(true)?;
    let mut workers: Vec<ConnectionWorker> = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        workers.retain(|w| w.thread.as_ref().is_some_and(|t| !t.is_finished()));
        if let Some(beats) = beats {
            beats();
        }
        match listener.accept() {
            Ok((sock, peer)) => {
                if workers.len() >= limits.connections {
                    continue; // dropped: too many viewers for a 64 MB camera
                }
                super::socket_limits::apply(&sock, 32 * 1024, 16 * 1024)?;
                sock.set_nonblocking(false)?;
                let socket = sock.try_clone()?;
                let cfg = Arc::clone(&cfg);
                let thread = limits.thread().spawn(move || {
                    let _ = connection(sock, peer, &cfg, limits);
                })?;
                workers.push(ConnectionWorker {
                    socket,
                    thread: Some(thread),
                });
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn nonce() -> String {
    let mut b = [0u8; 8];
    let ok = std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .is_ok();
    if !ok {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        b = t.to_le_bytes();
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

type Writer = Arc<Mutex<TcpStream>>;

/// `$ ch len16` then `parts`, written with as few syscalls as the kernel allows and no copy of the payload.
fn write_vectored_all(w: &mut TcpStream, parts: &mut [IoSlice<'_>]) -> io::Result<()> {
    let mut parts = parts;
    while !parts.is_empty() {
        match w.write_vectored(parts) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => IoSlice::advance_slices(&mut parts, n),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

enum Sink {
    Tcp { ch: u8 },
    Udp { sock: UdpSocket, to: SocketAddr },
}

struct Play {
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    /// One per sender: a sender writes nothing until `start` sends, so the PLAY response goes out first.
    go: Vec<mpsc::Sender<()>>,
}

impl Play {
    fn start(&mut self) {
        for go in self.go.drain(..) {
            let _ = go.send(());
        }
    }
}

impl Drop for Play {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Senders still waiting for the go-ahead see the channel close and return.
        self.go.clear();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

fn sender(
    viewer: Viewer,
    sink: Sink,
    w: Writer,
    mut pk: Packetizer,
    stop: Arc<AtomicBool>,
    go: mpsc::Receiver<()>,
) {
    if go.recv().is_err() {
        return;
    }
    let mut buf = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        let frame = match viewer.next(Duration::from_millis(200)) {
            Ok(Some(f)) => f,
            Ok(None) => continue,
            Err(_) => return,
        };
        let ts = timestamp(frame.pts_us);
        for p in pk.packets(&frame.data, ts) {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            let res = match &sink {
                Sink::Tcp { ch } => {
                    let len = (p.len() as u16).to_be_bytes();
                    let prefix = [b'$', *ch, len[0], len[1]];
                    let mut parts = [
                        IoSlice::new(&prefix),
                        IoSlice::new(p.head()),
                        IoSlice::new(p.payload),
                    ];
                    let mut g = w.lock().unwrap_or_else(|e| e.into_inner());
                    write_vectored_all(&mut g, &mut parts)
                }
                Sink::Udp { sock, to } => {
                    buf.clear();
                    buf.extend_from_slice(p.head());
                    buf.extend_from_slice(p.payload);
                    sock.send_to(&buf, to).map(|_| ())
                }
            };
            if res.is_err() {
                stop.store(true, Ordering::Relaxed);
                // Wake the control reader too: otherwise a failed sender can retain the
                // connection and its exclusive speaker lease until the idle timeout.
                let _ = w
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .shutdown(Shutdown::Both);
                return;
            }
        }
    }
}

enum Media {
    Video(Arc<Hub>),
    Audio(Arc<AudioHub>),
    Talk(TalkLease),
}

struct Setup {
    base: String,
    active: bool,
    media: Media,
    transport: Transport,
    udp: Option<UdpSocket>,
}

fn audio_sender(
    viewer: AudioViewer,
    sink: Sink,
    w: Writer,
    ssrc: u32,
    stop: Arc<AtomicBool>,
    go: mpsc::Receiver<()>,
) {
    if go.recv().is_err() {
        return;
    }
    let mut pk = crate::microphone::Packetizer::new(ssrc);
    let started = std::time::Instant::now();
    let mut pacer = crate::microphone::Pacer::default();
    while !stop.load(Ordering::Relaxed) {
        let frame = match viewer.next(Duration::from_millis(200)) {
            Ok(f) => f,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => return,
        };
        let Some(packet) = pk.packet(frame.pts_us, &frame.data) else {
            continue;
        };
        // Capture blocks (21.3 ms on the Reolink) yield frames unevenly; send them at a steady 20 ms.
        {
            let now = started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
            let Some(due) = pacer.deadline(now, crate::microphone::PACKET_US) else {
                return;
            };
            loop {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                let elapsed = started.elapsed().as_micros();
                if elapsed >= u128::from(due) {
                    break;
                }
                std::thread::sleep(Duration::from_micros(
                    (u128::from(due) - elapsed).min(5000) as u64
                ));
            }
            if stop.load(Ordering::Relaxed) {
                return;
            }
            let result = match &sink {
                Sink::Tcp { ch } => {
                    let len = (packet.len() as u16).to_be_bytes();
                    let prefix = [b'$', *ch, len[0], len[1]];
                    let mut parts = [IoSlice::new(&prefix), IoSlice::new(&packet)];
                    let mut guard = w.lock().unwrap_or_else(|e| e.into_inner());
                    write_vectored_all(&mut guard, &mut parts)
                }
                Sink::Udp { sock, to } => sock.send_to(&packet, to).map(|_| ()),
            };
            if result.is_err() {
                stop.store(true, Ordering::Relaxed);
                let _ = w
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .shutdown(Shutdown::Both);
                return;
            }
        }
    }
}

fn connection(
    mut sock: TcpStream,
    peer: SocketAddr,
    cfg: &Config,
    limits: ServerLimits,
) -> io::Result<()> {
    sock.set_read_timeout(Some(Duration::from_secs(SESSION_TIMEOUT_S + 30)))?;
    sock.set_write_timeout(Some(WRITE_TIMEOUT))?;
    sock.set_nodelay(true)?;
    let writer: Writer = Arc::new(Mutex::new(sock.try_clone()?));
    let nonce = nonce();
    let session_id = format!("{:016x}", u64::from_str_radix(&nonce, 16).unwrap_or(1));
    let mut setups: HashMap<String, Setup> = HashMap::new();
    let mut play: Option<Play> = None;
    let mut backchannel = false;
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        // Interleaved frames from the client (RTCP receiver reports) are skipped.
        while buf.first() == Some(&b'$') {
            if buf.len() < 4 {
                break;
            }
            let n = 4 + usize::from(u16::from_be_bytes([buf[2], buf[3]]));
            if buf.len() < n {
                break;
            }
            if play.is_some() {
                for setup in setups.values_mut() {
                    if setup.active
                        && matches!(setup.transport, Transport::Tcp { rtp_ch, .. } if rtp_ch == buf[1])
                        && let Media::Talk(input) = &mut setup.media
                    {
                        input.push_rtp(&buf[4..n]);
                    }
                }
            }
            buf.drain(..n);
        }
        let parsed = if buf.first() == Some(&b'$') {
            Parsed::Incomplete
        } else {
            parse_request(&buf)
        };
        match parsed {
            Parsed::Bad => return Ok(()),
            Parsed::Incomplete => {
                let n = sock.read(&mut chunk)?;
                if n == 0 {
                    return Ok(());
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            Parsed::Request(req, used) => {
                buf.drain(..used);
                let (resp, teardown) = handle(
                    &req,
                    cfg,
                    limits,
                    &nonce,
                    &session_id,
                    peer.ip(),
                    &mut setups,
                    &mut play,
                    &mut backchannel,
                    &writer,
                );
                let bytes = resp.encode(req.cseq());
                writer
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .write_all(&bytes)?;
                // Media follows the PLAY response, never precedes it.
                if let Some(playing) = play.as_mut() {
                    playing.start();
                }
                if teardown {
                    return Ok(());
                }
            }
        }
    }
}

fn find_stream<'a>(cfg: &'a Config, path: &str) -> Option<(&'a str, &'a Arc<Hub>)> {
    let base = path.split("/trackID=").next().unwrap_or(path);
    cfg.streams
        .iter()
        .find(|(p, _)| p == base)
        .map(|(p, h)| (p.as_str(), h))
}

#[allow(clippy::too_many_arguments)]
fn handle(
    req: &Request,
    cfg: &Config,
    limits: ServerLimits,
    nonce: &str,
    session_id: &str,
    peer_ip: IpAddr,
    setups: &mut HashMap<String, Setup>,
    play: &mut Option<Play>,
    backchannel: &mut bool,
    writer: &Writer,
) -> (Response, bool) {
    let public = "OPTIONS, DESCRIBE, SETUP, PLAY, TEARDOWN, GET_PARAMETER";
    if req.method == "OPTIONS" {
        return (Response::new(200).with("Public", public), false);
    }
    if let Some(c) = &cfg.creds {
        let ok = req
            .header("Authorization")
            .is_some_and(|h| check_digest(h, &req.method, &req.uri, c, nonce));
        if !ok {
            return (
                Response::new(401).with("WWW-Authenticate", challenge(nonce)),
                false,
            );
        }
    }
    let session = format!("{session_id};timeout={SESSION_TIMEOUT_S}");
    match req.method.as_str() {
        "DESCRIBE" => {
            let Some((_, hub)) = find_stream(cfg, req.path()) else {
                return (Response::new(404), false);
            };
            let Some(params) = hub.wait_params(DESCRIBE_WAIT) else {
                return (Response::new(404), false);
            };
            let base = format!("{}/", req.uri.trim_end_matches('/'));
            let mut body = sdp(&params, &base);
            if cfg.audio.is_some() {
                body.push_str(&crate::microphone::sdp());
            }
            if cfg.talk.is_some()
                && req.header("Require").is_some_and(|v| {
                    v.split(',')
                        .any(|v| v.trim() == "www.onvif.org/ver20/backchannel")
                })
            {
                *backchannel = true;
                body.push_str(&format!(
                    "m=audio 0 RTP/AVP {pt}\r\na=rtpmap:{pt} opus/48000/2\r\na=fmtp:{pt} sprop-stereo=0\r\n\
                     a=control:trackID=2\r\na=sendonly\r\n",
                    pt = crate::opus::PAYLOAD_TYPE
                ));
            }
            let r = Response::new(200)
                .with("Content-Base", base.clone())
                .body("application/sdp", body);
            (r, false)
        }
        "SETUP" => {
            // A running sender owns its negotiated channels/socket until PLAY is torn down.
            // Replacing the setup here would let another track reuse those live channels.
            if play.is_some() {
                return (Response::new(455), false);
            }
            let Some((name, hub)) = find_stream(cfg, req.path()) else {
                return (Response::new(404), false);
            };
            let track = req.path().split_once("/trackID=").map_or("0", |(_, t)| t);
            let media = match track {
                "0" => Media::Video(Arc::clone(hub)),
                "1" => match &cfg.audio {
                    Some(audio) => Media::Audio(Arc::clone(audio)),
                    None => return (Response::new(404), false),
                },
                "2" if *backchannel => match cfg.talk.as_ref().and_then(|t| t.claim()) {
                    Some(input) => Media::Talk(input),
                    None => return (Response::new(453), false),
                },
                _ => return (Response::new(404), false),
            };
            let Some(transport) = req.header("Transport").and_then(parse_transport) else {
                return (Response::new(461), false);
            };
            if matches!(media, Media::Talk(_)) && !matches!(transport, Transport::Tcp { .. }) {
                return (Response::new(461), false);
            }
            if let Transport::Tcp { rtp_ch, rtcp_ch } = &transport {
                let key = format!("{name}/{track}");
                if rtp_ch == rtcp_ch
                    || setups.iter().any(|(k, s)| {
                        k != &key
                            && match &s.transport {
                                Transport::Tcp {
                                    rtp_ch: a,
                                    rtcp_ch: b,
                                } => [a, b].iter().any(|c| *c == rtp_ch || *c == rtcp_ch),
                                _ => false,
                            }
                    })
                {
                    return (Response::new(461), false);
                }
            }
            let (reply, udp) = match &transport {
                Transport::Tcp { rtp_ch, rtcp_ch } => (
                    format!("RTP/AVP/TCP;unicast;interleaved={rtp_ch}-{rtcp_ch}"),
                    None,
                ),
                Transport::Udp {
                    rtp_port,
                    rtcp_port,
                } => {
                    let Ok(s) = UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, 0)) else {
                        return (Response::new(461), false);
                    };
                    if super::socket_limits::apply(&s, 16 * 1024, 8 * 1024).is_err()
                        || s.set_write_timeout(Some(WRITE_TIMEOUT)).is_err()
                    {
                        return (Response::new(461), false);
                    }
                    let port = s.local_addr().map_or(0, |a| a.port());
                    (
                        format!(
                            "RTP/AVP;unicast;client_port={rtp_port}-{rtcp_port};server_port={port}-{}",
                            port.saturating_add(1)
                        ),
                        Some(s),
                    )
                }
            };
            setups.insert(
                format!("{name}/{track}"),
                Setup {
                    base: name.to_string(),
                    active: false,
                    media,
                    transport,
                    udp,
                },
            );
            (
                Response::new(200)
                    .with("Transport", reply)
                    .with("Session", session),
                false,
            )
        }
        "PLAY" => {
            let Some((base, _)) = find_stream(cfg, req.path()) else {
                return (Response::new(404), false);
            };
            if !setups.values().any(|s| s.base == base) {
                return (Response::new(455), false);
            }
            // Drop the previous senders before replacing PLAY, including repeated PLAY requests.
            *play = None;
            let stop = Arc::new(AtomicBool::new(false));
            let mut playing = Play {
                stop: Arc::clone(&stop),
                threads: Vec::new(),
                go: Vec::new(),
            };
            for setup in setups.values_mut() {
                setup.active = setup.base == base;
            }
            for (index, setup) in setups.values().filter(|s| s.active).enumerate() {
                if matches!(setup.media, Media::Talk(_)) {
                    continue;
                }
                let sink = match (&setup.transport, &setup.udp) {
                    (Transport::Tcp { rtp_ch, .. }, _) => Sink::Tcp { ch: *rtp_ch },
                    (Transport::Udp { rtp_port, .. }, Some(s)) => match s.try_clone() {
                        Ok(sock) => Sink::Udp {
                            sock,
                            to: SocketAddr::new(peer_ip, *rtp_port),
                        },
                        Err(_) => return (Response::new(455), false),
                    },
                    _ => return (Response::new(455), false),
                };
                let ssrc = u32::from_str_radix(&nonce[..8.min(nonce.len())], 16)
                    .unwrap_or(1)
                    .wrapping_add(index as u32);
                let (w, st) = (Arc::clone(writer), Arc::clone(&stop));
                let (go_tx, go_rx) = mpsc::channel();
                playing.go.push(go_tx);
                match &setup.media {
                    Media::Video(hub) => {
                        let viewer = hub.subscribe();
                        let pk = Packetizer::new(ssrc, 0, MAX_PAYLOAD);
                        match limits
                            .thread()
                            .spawn(move || sender(viewer, sink, w, pk, st, go_rx))
                        {
                            Ok(thread) => playing.threads.push(thread),
                            Err(_) => return (Response::new(455), false),
                        }
                    }
                    Media::Audio(hub) => {
                        let rx = hub.subscribe();
                        match limits
                            .thread()
                            .spawn(move || audio_sender(rx, sink, w, ssrc, st, go_rx))
                        {
                            Ok(thread) => playing.threads.push(thread),
                            Err(_) => return (Response::new(455), false),
                        }
                    }
                    Media::Talk(_) => {}
                }
            }
            *play = Some(playing);
            (
                Response::new(200)
                    .with("Range", "npt=0.000-")
                    .with("Session", session),
                false,
            )
        }
        "GET_PARAMETER" | "SET_PARAMETER" => (Response::new(200).with("Session", session), false),
        "TEARDOWN" => {
            *play = None;
            setups.clear();
            (Response::new(200).with("Session", session), true)
        }
        _ => (Response::new(455), false),
    }
}

#[cfg(test)]
mod tests;
