//! Wiring every camera shares: the identity read from the running device, and the HTTP and discovery
//! workers started and stopped together.

use crate::camera::{DeviceIdentity, OnvifCamera};
use crate::http::{self, Events, HttpLimits, OnvifServer};
use crate::netif;
use crate::udp::Discovery;
use camera_transport::Credentials;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

/// What a camera knows about itself at build time; the rest is read from the device.
#[derive(Debug, Clone, Default)]
pub struct Product {
    pub manufacturer: String,
    pub model: String,
    pub firmware: String,
    pub hardware: String,
    /// The owner's name for this camera.
    pub name: String,
    /// NTP servers the camera's clock follows.
    pub ntp: Vec<String>,
    /// Whether the address comes from DHCP.
    pub dhcp: bool,
}

/// The identity of the running device: its first IPv4 interface that is up (or `interface` when
/// given), that interface's MAC as serial and endpoint UUID, DNS servers and hostname.
pub fn identity(product: Product, interface: Option<&str>) -> DeviceIdentity {
    let all = netif::interfaces();
    let nic = match interface {
        Some(name) => all.into_iter().find(|i| i.name == name),
        None => all.into_iter().next(),
    };
    let mac = nic.as_ref().and_then(|n| n.mac);
    DeviceIdentity {
        manufacturer: product.manufacturer,
        model: product.model,
        firmware: product.firmware,
        hardware: product.hardware,
        uuid: mac.map_or_else(
            // No MAC: a per-boot UUID still lets clients find the camera.
            || crate::codec::uuid_v4(crate::entropy::random::<16>()),
            |m| DeviceIdentity::endpoint(&m),
        ),
        name: product.name,
        hostname: netif::hostname(),
        interface: nic.as_ref().map_or_else(String::new, |n| n.name.clone()),
        mac,
        prefix_len: nic.as_ref().map_or(0, |n| n.prefix_len),
        dhcp: product.dhcp,
        dns: netif::dns(),
        ntp: product.ntp,
    }
}

/// A running ONVIF service. Dropping it stops discovery (sending `Bye`) and the HTTP server.
pub struct Service {
    server: Arc<OnvifServer>,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl Service {
    /// Where the camera reports digital input edges.
    pub fn events(&self) -> Arc<Events> {
        self.server.events()
    }

    pub fn snapshot_encodes(&self) -> u64 {
        self.server.snapshot_encodes()
    }

    pub fn server(&self) -> &Arc<OnvifServer> {
        &self.server
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

/// Starts the HTTP service on `http_port` and WS-Discovery. `log` receives one line per worker that ends.
pub fn start(
    camera: Arc<dyn OnvifCamera>,
    creds: Credentials,
    rtsp_port: u16,
    http_port: u16,
    limits: HttpLimits,
    log: fn(String),
) -> io::Result<Service> {
    let listener = http::bind(http_port)?;
    let discovery = Discovery {
        identity: camera.identity().clone(),
        ptz: camera.ptz().is_some(),
        http_port,
    };
    let server = OnvifServer::new(camera, creds, rtsp_port, limits);
    let stop = Arc::new(AtomicBool::new(false));
    let mut threads = Vec::new();
    let (s, srv) = (Arc::clone(&stop), Arc::clone(&server));
    threads.push(
        std::thread::Builder::new()
            .name("onvif".into())
            .stack_size(128 * 1024)
            .spawn(move || {
                let result = http::serve(&listener, srv, &s);
                log(format!("onvif: http ended {result:?}"));
            })?,
    );
    let s = Arc::clone(&stop);
    threads.push(
        std::thread::Builder::new()
            .name("onvif-discovery".into())
            .stack_size(64 * 1024)
            .spawn(move || {
                let result = discovery.run(&s);
                log(format!("onvif: discovery ended {result:?}"));
            })?,
    );
    Ok(Service {
        server,
        stop,
        threads,
    })
}
