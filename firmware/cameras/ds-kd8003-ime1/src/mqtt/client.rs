//! One blocking MQTT connection: connect, subscribe, publish, keep alive, read incoming publishes.
use super::packet::{self, Decode, Packet, Will};
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

pub struct Options {
    pub client_id: String,
    pub username: String,
    pub password: String,
    pub keep_alive: Duration,
    pub will_topic: String,
    pub will_payload: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Incoming {
    Publish {
        topic: String,
        payload: Vec<u8>,
        /// Set when the broker delivers a message it had retained (not for live publishes).
        retain: bool,
    },
}

pub struct Client {
    socket: TcpStream,
    buf: Vec<u8>,
    keep_alive: Duration,
    last_sent: Instant,
    last_received: Instant,
    ping_outstanding: bool,
    next_id: u16,
}

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

impl Client {
    /// Connects, sends CONNECT and waits for a successful CONNACK.
    pub fn connect(addr: impl ToSocketAddrs, options: &Options) -> io::Result<Self> {
        let target = addr
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::other("no broker address"))?;
        let socket = TcpStream::connect_timeout(&target, CONNECT_TIMEOUT)?;
        socket.set_nodelay(true)?;
        socket.set_write_timeout(Some(CONNECT_TIMEOUT))?;
        socket.set_read_timeout(Some(Duration::from_millis(200)))?;
        let mut client = Self {
            socket,
            buf: Vec::with_capacity(4096),
            keep_alive: options.keep_alive.max(Duration::from_secs(5)),
            last_sent: Instant::now(),
            last_received: Instant::now(),
            ping_outstanding: false,
            next_id: 1,
        };
        let user = (!options.username.is_empty())
            .then_some((options.username.as_str(), options.password.as_str()));
        let will = (!options.will_topic.is_empty()).then_some(Will {
            topic: &options.will_topic,
            payload: &options.will_payload,
            retain: true,
        });
        client.send(&packet::connect(
            &options.client_id,
            client.keep_alive.as_secs().min(u64::from(u16::MAX)) as u16,
            user,
            will,
        ))?;
        let deadline = Instant::now() + CONNECT_TIMEOUT;
        loop {
            match client.read_packet(deadline)? {
                Some(Packet::ConnAck { code: 0, .. }) => return Ok(client),
                Some(Packet::ConnAck { code, .. }) => {
                    return Err(io::Error::other(format!(
                        "broker refused connection: {code}"
                    )));
                }
                Some(_) => return Err(io::Error::other("unexpected packet before CONNACK")),
                None => {
                    if Instant::now() >= deadline {
                        return Err(io::Error::new(io::ErrorKind::TimedOut, "no CONNACK"));
                    }
                }
            }
        }
    }

    fn send(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.socket.write_all(bytes)?;
        self.last_sent = Instant::now();
        Ok(())
    }

    pub fn publish(&mut self, topic: &str, payload: &[u8], retain: bool) -> io::Result<()> {
        self.send(&packet::publish(topic, payload, retain))
    }

    /// QoS 0 subscriptions; the SUBACK is consumed by `poll`.
    pub fn subscribe(&mut self, topics: &[&str]) -> io::Result<()> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.send(&packet::subscribe(id, topics))
    }

    /// Reads one packet or returns `None` after the read timeout / at `deadline`.
    fn read_packet(&mut self, deadline: Instant) -> io::Result<Option<Packet>> {
        loop {
            match packet::decode(&self.buf) {
                Decode::Packet(p, used) => {
                    self.buf.drain(..used);
                    self.last_received = Instant::now();
                    return Ok(Some(p));
                }
                Decode::Malformed => return Err(io::Error::other("malformed MQTT packet")),
                Decode::Incomplete => {}
            }
            if self.buf.len() > 64 * 1024 {
                return Err(io::Error::other("oversized MQTT packet"));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
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

    /// Services the connection for up to `wait`: delivers one incoming publish, answers keep-alive.
    /// An error means the connection is dead and must be re-established.
    pub fn poll(&mut self, wait: Duration) -> io::Result<Option<Incoming>> {
        let deadline = Instant::now() + wait;
        loop {
            let idle = self.last_sent.elapsed();
            if self.ping_outstanding && self.last_received.elapsed() > self.keep_alive {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "keep-alive expired",
                ));
            }
            if idle >= self.keep_alive / 2 && !self.ping_outstanding {
                self.send(&packet::pingreq())?;
                self.ping_outstanding = true;
            }
            match self.read_packet(deadline)? {
                Some(Packet::Publish {
                    topic,
                    payload,
                    retain,
                }) => {
                    return Ok(Some(Incoming::Publish {
                        topic,
                        payload,
                        retain,
                    }));
                }
                Some(Packet::SubAck { codes, .. }) if codes.iter().any(|c| *c & 0x80 != 0) => {
                    return Err(io::Error::other("broker refused a subscription"));
                }
                Some(Packet::PingResp) => self.ping_outstanding = false,
                Some(_) => {}
                None => {
                    if Instant::now() >= deadline {
                        return Ok(None);
                    }
                }
            }
        }
    }

    pub fn disconnect(mut self) {
        let _ = self.send(&packet::disconnect());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// A loopback broker fixture that accepts one client; nothing here reaches a real broker.
    fn broker() -> (TcpListener, std::net::SocketAddr) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let a = l.local_addr().unwrap();
        (l, a)
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
    fn connects_subscribes_publishes_and_receives() {
        let (listener, addr) = broker();
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
            s.write_all(&packet::publish("device/door/command", b"open", false))
                .unwrap();
            let ping = read_frame(&mut s);
            assert_eq!(ping, [0xc0, 0]);
            s.write_all(&[0xd0, 0]).unwrap();
            std::thread::sleep(Duration::from_millis(1500));
        });
        let options = Options {
            client_id: "test".into(),
            username: String::new(),
            password: String::new(),
            keep_alive: Duration::from_secs(5),
            will_topic: "device/availability".into(),
            will_payload: b"offline".to_vec(),
        };
        let mut c = Client::connect(addr, &options).unwrap();
        c.subscribe(&["device/door/command"]).unwrap();
        c.publish("device/availability", b"online", true).unwrap();
        let incoming = c.poll(Duration::from_secs(2)).unwrap();
        assert_eq!(
            incoming,
            Some(Incoming::Publish {
                topic: "device/door/command".into(),
                payload: b"open".to_vec(),
                retain: false
            })
        );
        c.last_sent = Instant::now() - Duration::from_secs(3);
        assert_eq!(c.poll(Duration::from_millis(500)).unwrap(), None);
        assert!(!c.ping_outstanding, "PINGRESP was consumed");
        server.join().unwrap();
    }

    #[test]
    fn a_refused_connack_and_a_closed_socket_are_errors() {
        let (listener, addr) = broker();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            read_frame(&mut s);
            s.write_all(&[0x20, 2, 0, 5]).unwrap();
        });
        let options = Options {
            client_id: "t".into(),
            username: "u".into(),
            password: "p".into(),
            keep_alive: Duration::from_secs(5),
            will_topic: String::new(),
            will_payload: Vec::new(),
        };
        assert!(Client::connect(addr, &options).is_err());
        let (listener, addr) = broker();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            read_frame(&mut s);
            s.write_all(&[0x20, 2, 0, 0]).unwrap();
        });
        let mut c = Client::connect(addr, &options).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            c.poll(Duration::from_millis(300)).is_err(),
            "broker went away"
        );
    }
}
