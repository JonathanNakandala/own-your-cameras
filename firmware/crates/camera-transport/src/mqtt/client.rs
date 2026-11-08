//! The worker's one blocking MQTT 3.1.1 connection: clean session, QoS 0 publishes and
//! subscriptions, a retained QoS 1 last will, keep-alive. It connects on demand and reports every
//! control packet the worker acts on (CONNACK, SUBACK, PUBLISH) as one poll result.
use super::packet::{self, Decode, Packet, Will};
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

pub(super) struct Options {
    pub host: String,
    pub port: u16,
    pub client_id: String,
    pub username: String,
    pub password: String,
    pub keep_alive: Duration,
    pub will_topic: String,
    pub will_payload: Vec<u8>,
    /// Largest incoming packet; a bigger one is read past and discarded.
    pub max_packet: usize,
}

#[derive(Debug)]
pub(super) enum Polled {
    ConnAck {
        session_present: bool,
    },
    /// One subscription acknowledged; `refused` when the broker answered with failure (0x80).
    SubAck {
        refused: bool,
    },
    Publish {
        topic: String,
        payload: Vec<u8>,
        retain: bool,
    },
    /// An incoming packet over `max_packet` (its remaining length) was read past and discarded.
    Oversized(usize),
    /// Nothing the worker acts on arrived within the wait.
    Idle,
    /// No connection: it failed, was refused or closed. The next poll connects again.
    Lost(io::Error),
}

struct Connection {
    socket: TcpStream,
    buf: Vec<u8>,
    last_sent: Instant,
    last_received: Instant,
    /// When the unanswered PINGREQ went out.
    ping_sent: Option<Instant>,
    /// Bytes of a discarded oversized packet still to arrive.
    skip: usize,
}

enum Incoming {
    Packet(Packet),
    /// An oversized packet's remaining length; its bytes are discarded.
    Oversized(usize),
}

pub(super) struct Client {
    options: Options,
    connection: Option<Connection>,
    /// A write failed; the next poll reports the connection lost.
    failed: Option<io::Error>,
    next_id: u16,
}

/// A blocked write longer than this breaks the connection.
const WRITE_TIMEOUT: Duration = Duration::from_secs(1);
/// After DISCONNECT, how long unread input is drained before the socket closes.
const DRAIN: Duration = Duration::from_secs(2);

impl Connection {
    fn send(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.socket.write_all(bytes)?;
        self.last_sent = Instant::now();
        Ok(())
    }

    /// Reads one packet, or `None` at `deadline`. A packet over `max` is not buffered: its bytes
    /// are discarded as they arrive and the connection stays up, so a retained oversized message
    /// cannot force a reconnect loop.
    fn read_packet(&mut self, deadline: Instant, max: usize) -> io::Result<Option<Incoming>> {
        loop {
            if self.skip > 0 {
                let n = self.skip.min(self.buf.len());
                self.buf.drain(..n);
                self.skip -= n;
            }
            if self.skip == 0 {
                if let Some((header, total)) = packet::frame_len(&self.buf)
                    && total - header > max
                {
                    let n = total.min(self.buf.len());
                    self.buf.drain(..n);
                    self.skip = total - n;
                    self.last_received = Instant::now();
                    return Ok(Some(Incoming::Oversized(total - header)));
                }
                match packet::decode(&self.buf) {
                    Decode::Packet(p, used) => {
                        self.buf.drain(..used);
                        self.last_received = Instant::now();
                        return Ok(Some(Incoming::Packet(p)));
                    }
                    Decode::Malformed => return Err(io::Error::other("malformed MQTT packet")),
                    Decode::Incomplete => {}
                }
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            self.socket
                .set_read_timeout(Some(left.max(Duration::from_millis(1))))?;
            let mut chunk = [0u8; 2048];
            match self.socket.read(&mut chunk) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionAborted,
                        "broker closed",
                    ));
                }
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::Interrupted
                    ) =>
                {
                    return Ok(None);
                }
                Err(e) => return Err(e),
            }
        }
    }
}

impl Client {
    pub fn new(options: Options) -> Self {
        Self {
            options,
            connection: None,
            failed: None,
            next_id: 1,
        }
    }

    fn id(&mut self) -> u16 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        id
    }

    /// Opens the socket and sends CONNECT; the CONNACK must arrive by `deadline`.
    fn connect(&mut self, deadline: Instant) -> io::Result<(Connection, bool)> {
        let o = &self.options;
        let target = (o.host.as_str(), o.port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::other("no broker address"))?;
        let wait = deadline
            .saturating_duration_since(Instant::now())
            .max(Duration::from_millis(1));
        let socket = TcpStream::connect_timeout(&target, wait)?;
        socket.set_nodelay(true)?;
        socket.set_write_timeout(Some(WRITE_TIMEOUT))?;
        let mut c = Connection {
            socket,
            buf: Vec::with_capacity(4096),
            last_sent: Instant::now(),
            last_received: Instant::now(),
            ping_sent: None,
            skip: 0,
        };
        let user = (!o.username.is_empty()).then_some((o.username.as_str(), o.password.as_str()));
        let will = (!o.will_topic.is_empty()).then_some(Will {
            topic: &o.will_topic,
            payload: &o.will_payload,
        });
        c.send(&packet::connect(
            &o.client_id,
            o.keep_alive.as_secs().min(u64::from(u16::MAX)) as u16,
            user,
            will,
        )?)?;
        loop {
            match c.read_packet(deadline, o.max_packet)? {
                Some(Incoming::Packet(Packet::ConnAck {
                    code: 0,
                    session_present,
                })) => return Ok((c, session_present)),
                Some(Incoming::Packet(Packet::ConnAck { code, .. })) => {
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionRefused,
                        format!("broker refused connection: {code}"),
                    ));
                }
                Some(_) => return Err(io::Error::other("unexpected packet before CONNACK")),
                None if Instant::now() >= deadline => {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "no CONNACK"));
                }
                None => {}
            }
        }
    }

    /// False when there is no working connection or the packet could not be encoded.
    fn send_encoded(&mut self, encoded: io::Result<Vec<u8>>) -> bool {
        encoded.is_ok_and(|bytes| self.send(&bytes))
    }

    fn send(&mut self, bytes: &[u8]) -> bool {
        let Some(c) = self.connection.as_mut().filter(|_| self.failed.is_none()) else {
            return false;
        };
        match c.send(bytes) {
            Ok(()) => true,
            Err(e) => {
                self.failed = Some(e);
                false
            }
        }
    }

    /// QoS 0. False when there is no working connection (or the topic is over 64 KiB, which a
    /// validated [`super::Config`] cannot produce); the publication was not sent.
    pub fn publish(&mut self, topic: &str, payload: &[u8], retain: bool) -> bool {
        self.send_encoded(packet::publish(topic, payload, retain))
    }

    /// One QoS 0 subscription per SUBSCRIBE, so each one reports its own SUBACK.
    pub fn subscribe(&mut self, filter: &str) -> bool {
        let id = self.id();
        self.send_encoded(packet::subscribe(id, &[filter]))
    }

    pub fn unsubscribe(&mut self, filter: &str) -> bool {
        let id = self.id();
        self.send_encoded(packet::unsubscribe(id, &[filter]))
    }

    /// Connects when there is no connection (`wait` bounds TCP connect and CONNACK); otherwise
    /// services the connection for up to `wait`, answering keep-alive, and returns the first
    /// packet the worker acts on.
    pub fn poll(&mut self, wait: Duration) -> Polled {
        let deadline = Instant::now() + wait;
        if let Some(e) = self.failed.take() {
            self.connection = None;
            return Polled::Lost(e);
        }
        let Some(c) = self.connection.as_mut() else {
            return match self.connect(deadline) {
                Ok((c, session_present)) => {
                    self.connection = Some(c);
                    Polled::ConnAck { session_present }
                }
                Err(e) => Polled::Lost(e),
            };
        };
        let keep_alive = self.options.keep_alive;
        let result = (|| loop {
            if c.ping_sent.is_some_and(|at| at.elapsed() > keep_alive) {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "keep-alive expired",
                ));
            }
            // Ping on a quiet input too: steady publishing alone cannot reveal a dead broker.
            let quiet = c.last_sent.elapsed().max(c.last_received.elapsed()) >= keep_alive / 2;
            if quiet && c.ping_sent.is_none() {
                c.send(&packet::pingreq())?;
                c.ping_sent = Some(Instant::now());
            }
            match c.read_packet(deadline, self.options.max_packet)? {
                Some(Incoming::Packet(Packet::Publish {
                    topic,
                    payload,
                    retain,
                })) => {
                    return Ok(Polled::Publish {
                        topic,
                        payload,
                        retain,
                    });
                }
                Some(Incoming::Packet(Packet::SubAck { codes, .. })) => {
                    return Ok(Polled::SubAck {
                        refused: codes.contains(&0x80),
                    });
                }
                Some(Incoming::Packet(Packet::PingResp)) => c.ping_sent = None,
                Some(Incoming::Oversized(len)) => return Ok(Polled::Oversized(len)),
                Some(Incoming::Packet(_)) => {}
                None if Instant::now() >= deadline => return Ok(Polled::Idle),
                None => {}
            }
        })();
        result.unwrap_or_else(|e| {
            self.connection = None;
            Polled::Lost(e)
        })
    }

    /// Forgets the connection and anything not yet sent.
    pub fn reset(&mut self) {
        self.connection = None;
        self.failed = None;
    }

    /// Publishes `payload` retained on `topic`, sends DISCONNECT and closes after the broker has
    /// had the chance to read everything: TCP keeps the order, and unread input is drained first
    /// so closing cannot reset the connection over the final writes. The next poll connects again.
    pub fn close(&mut self, topic: &str, payload: &[u8]) {
        let _ = self.publish(topic, payload, true);
        if self.send(&packet::disconnect()) {
            self.drain();
        }
        self.reset();
    }

    fn drain(&mut self) {
        let Some(c) = self.connection.as_mut() else {
            return;
        };
        let _ = c.socket.shutdown(Shutdown::Write);
        let deadline = Instant::now() + DRAIN;
        let mut chunk = [0u8; 2048];
        while Instant::now() < deadline {
            let _ = c.socket.set_read_timeout(Some(Duration::from_millis(100)));
            match c.socket.read(&mut chunk) {
                Ok(n) if n > 0 => {}
                // EOF, an error, or no further input: nothing left to reset over.
                _ => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// A loopback broker fixture that accepts one client; nothing here reaches a real broker.
    fn broker() -> (TcpListener, u16) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        (l, port)
    }

    fn options(port: u16, user: &str) -> Options {
        Options {
            host: "127.0.0.1".into(),
            port,
            client_id: "test".into(),
            username: user.into(),
            password: if user.is_empty() { "" } else { "p" }.into(),
            keep_alive: Duration::from_secs(5),
            will_topic: "device/availability".into(),
            will_payload: b"offline".to_vec(),
            max_packet: 8192,
        }
    }

    fn read_frame(s: &mut TcpStream) -> Vec<u8> {
        let mut head = [0u8; 2];
        s.read_exact(&mut head).unwrap();
        let mut rest = vec![0u8; head[1] as usize];
        s.read_exact(&mut rest).unwrap();
        let mut out = head.to_vec();
        out.extend(rest);
        out
    }

    #[test]
    fn connects_subscribes_publishes_receives_and_closes() {
        let (listener, port) = broker();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let connect = read_frame(&mut s);
            assert_eq!(connect[0], 0x10);
            s.write_all(&[0x20, 2, 0, 0]).unwrap();
            let sub = read_frame(&mut s);
            assert_eq!(sub[0], 0x82);
            s.write_all(&[0x90, 3, sub[2], sub[3], 0]).unwrap();
            let publish = read_frame(&mut s);
            assert_eq!(publish[0], 0x31);
            s.write_all(&packet::publish("device/door/command", b"open", false).unwrap())
                .unwrap();
            let ping = read_frame(&mut s);
            assert_eq!(ping, [0xc0, 0]);
            s.write_all(&[0xd0, 0]).unwrap();
            let offline = read_frame(&mut s);
            assert_eq!(offline[0], 0x31);
            assert_eq!(read_frame(&mut s), [0xe0, 0]);
            let mut rest = Vec::new();
            s.read_to_end(&mut rest).unwrap();
            assert!(rest.is_empty(), "client closed after DISCONNECT");
        });
        let mut c = Client::new(options(port, ""));
        assert!(matches!(
            c.poll(Duration::from_secs(2)),
            Polled::ConnAck {
                session_present: false
            }
        ));
        assert!(c.subscribe("device/door/command"));
        assert!(matches!(
            c.poll(Duration::from_secs(2)),
            Polled::SubAck { refused: false }
        ));
        assert!(c.publish("device/availability", b"online", true));
        assert!(matches!(
            c.poll(Duration::from_secs(2)),
            Polled::Publish { topic, payload, retain: false }
                if topic == "device/door/command" && payload == b"open"
        ));
        c.connection.as_mut().unwrap().last_sent = Instant::now() - Duration::from_secs(3);
        assert!(matches!(c.poll(Duration::from_millis(500)), Polled::Idle));
        assert!(
            c.connection.as_ref().unwrap().ping_sent.is_none(),
            "PINGRESP was consumed"
        );
        c.close("device/availability", b"offline");
        server.join().unwrap();
    }

    #[test]
    fn a_refused_connack_and_a_closed_socket_are_lost_connections() {
        let (listener, port) = broker();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            read_frame(&mut s);
            s.write_all(&[0x20, 2, 0, 5]).unwrap();
        });
        let mut c = Client::new(options(port, "u"));
        assert!(matches!(c.poll(Duration::from_secs(2)), Polled::Lost(_)));
        let (listener, port) = broker();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            read_frame(&mut s);
            s.write_all(&[0x20, 2, 0, 0]).unwrap();
        });
        let mut c = Client::new(options(port, "u"));
        assert!(matches!(
            c.poll(Duration::from_secs(2)),
            Polled::ConnAck { .. }
        ));
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            matches!(c.poll(Duration::from_millis(300)), Polled::Lost(_)),
            "broker went away"
        );
        assert!(!c.publish("t", b"x", false), "no connection to publish on");
    }

    #[test]
    fn an_oversized_packet_is_discarded_without_dropping_the_connection() {
        let (listener, port) = broker();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            read_frame(&mut s);
            s.write_all(&[0x20, 2, 0, 0]).unwrap();
            let big = packet::publish("device/door/command", &vec![b'x'; 20_000], true).unwrap();
            // In pieces, so the discard spans several reads.
            for piece in big.chunks(3000) {
                s.write_all(piece).unwrap();
                std::thread::sleep(Duration::from_millis(5));
            }
            s.write_all(&packet::publish("device/door/command", b"open", false).unwrap())
                .unwrap();
            s.write_all(&[0x90, 3, 0, 1, 0x80]).unwrap();
            let mut rest = Vec::new();
            let _ = s.read_to_end(&mut rest);
        });
        let mut c = Client::new(options(port, ""));
        assert!(matches!(
            c.poll(Duration::from_secs(2)),
            Polled::ConnAck { .. }
        ));
        assert!(matches!(
            c.poll(Duration::from_secs(2)),
            Polled::Oversized(n) if n > 20_000
        ));
        assert!(matches!(
            c.poll(Duration::from_secs(2)),
            Polled::Publish { payload, .. } if payload == b"open"
        ));
        assert!(matches!(
            c.poll(Duration::from_secs(2)),
            Polled::SubAck { refused: true }
        ));
        c.close("device/availability", b"offline");
        server.join().unwrap();
    }
}
