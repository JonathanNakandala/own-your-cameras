//! The WS-Discovery adapter: listens on UDP 3702 in the 239.255.255.250 group, answers matching probes
//! by unicast, sends `Hello` when it starts and `Bye` when it stops cleanly.

use crate::DeviceIdentity;
use crate::discovery::{self, MULTICAST, PORT};
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const MAX_DATAGRAM: usize = 8 * 1024;

fn message_id() -> String {
    crate::codec::uuid_v4(crate::entropy::random::<16>())
}

/// The address this host uses to reach `peer`: the interface a reply leaves by, so the XAddr a client
/// gets is one it can reach (no interface enumeration needed).
fn local_toward(peer: SocketAddr) -> Option<IpAddr> {
    let probe = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    probe.connect(peer).ok()?;
    Some(probe.local_addr().ok()?.ip())
}

fn xaddr(ip: IpAddr, http_port: u16) -> String {
    format!("http://{ip}:{http_port}/onvif/device_service")
}

pub struct Discovery {
    pub identity: DeviceIdentity,
    pub ptz: bool,
    pub http_port: u16,
}

impl Discovery {
    /// Binds 3702, joins the group, announces, serves until `stop`, then says goodbye. The join is retried
    /// until it works: at boot the camera starts before Wi-Fi has an address, and the kernel refuses a
    /// multicast join without a route (`ENODEV`).
    pub fn run(&self, stop: &AtomicBool) -> io::Result<()> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, PORT))?;
        while let Err(e) = socket.join_multicast_v4(&MULTICAST, &Ipv4Addr::UNSPECIFIED) {
            if e.raw_os_error() != Some(libc::ENODEV) {
                return Err(e);
            }
            for _ in 0..10 {
                if stop.load(std::sync::atomic::Ordering::Relaxed) {
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        }
        socket.set_multicast_loop_v4(false)?;
        let group = SocketAddr::from((MULTICAST, PORT));
        if let Some(ip) = local_toward(group) {
            let scopes = discovery::scopes(&self.identity, self.ptz);
            let hello = discovery::hello(
                &self.identity,
                &scopes,
                &xaddr(ip, self.http_port),
                &message_id(),
            );
            let _ = socket.send_to(&hello, group);
        }
        let result = self.serve(&socket, stop);
        let _ = socket.send_to(&discovery::bye(&self.identity, &message_id()), group);
        result
    }

    /// Answers probes on `socket` until `stop`.
    pub fn serve(&self, socket: &UdpSocket, stop: &AtomicBool) -> io::Result<()> {
        socket.set_read_timeout(Some(Duration::from_millis(500)))?;
        let scopes = discovery::scopes(&self.identity, self.ptz);
        let mut buf = vec![0u8; MAX_DATAGRAM];
        while !stop.load(Ordering::Relaxed) {
            let (n, peer) = match socket.recv_from(&mut buf) {
                Ok(r) => r,
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    continue;
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            let Some(ip) = local_toward(peer) else {
                continue;
            };
            if let Some(reply) = discovery::probe_match(
                &buf[..n],
                &self.identity,
                &scopes,
                &xaddr(ip, self.http_port),
                &message_id(),
            ) {
                let _ = socket.send_to(&reply, peer);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn a_unicast_probe_on_loopback_gets_a_match_naming_the_reachable_address() {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = socket.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let s = Arc::clone(&stop);
        let worker = std::thread::spawn(move || {
            let d = Discovery {
                identity: DeviceIdentity {
                    model: "Model X".into(),
                    name: "Test".into(),
                    uuid: "00000000-0000-5000-8000-000000000001".into(),
                    ..DeviceIdentity::default()
                },
                ptz: false,
                http_port: 8080,
            };
            d.serve(&socket, &s)
        });
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let probe = br#"<?xml version="1.0"?><e:Envelope xmlns:e="http://www.w3.org/2003/05/soap-envelope" xmlns:w="http://schemas.xmlsoap.org/ws/2004/08/addressing" xmlns:d="http://schemas.xmlsoap.org/ws/2005/04/discovery" xmlns:dn="http://www.onvif.org/ver10/network/wsdl"><e:Header><w:MessageID>uuid:1</w:MessageID></e:Header><e:Body><d:Probe><d:Types>dn:NetworkVideoTransmitter</d:Types></d:Probe></e:Body></e:Envelope>"#;
        client.send_to(probe, addr).unwrap();
        let mut buf = [0u8; 8192];
        let (n, _) = client.recv_from(&mut buf).unwrap();
        let reply = String::from_utf8_lossy(&buf[..n]).to_string();
        assert!(
            reply.contains("<d:XAddrs>http://127.0.0.1:8080/onvif/device_service</d:XAddrs>"),
            "{reply}"
        );
        assert!(reply.contains("<wsa:RelatesTo>uuid:1</wsa:RelatesTo>"));
        client.send_to(b"garbage", addr).unwrap();
        assert!(client.recv_from(&mut buf).is_err(), "no reply to garbage");
        stop.store(true, Ordering::Relaxed);
        worker.join().unwrap().unwrap();
    }
}
