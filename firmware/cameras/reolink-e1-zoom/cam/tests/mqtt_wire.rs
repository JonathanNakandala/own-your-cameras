//! Isolated MQTT peer: no shared broker or camera state is changed by these tests.
use cam::mqtt::{self, Command, Config, Hardware, PipelineHealth};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
struct FakeHardware;
impl Hardware for FakeHardware {
    fn apply(&mut self, _: &Command) -> Result<(), String> {
        Ok(())
    }
    fn tick(&mut self, _: Duration) -> Result<Option<Duration>, String> {
        Ok(None)
    }
    fn states(&mut self) -> Result<BTreeMap<String, String>, String> {
        Ok(BTreeMap::from([("temperature".into(), "56".into())]))
    }
}
fn packet(s: &mut TcpStream) -> (u8, Vec<u8>) {
    let mut b = [0];
    s.read_exact(&mut b).unwrap();
    let head = b[0];
    let (mut len, mut shift) = (0, 0);
    loop {
        s.read_exact(&mut b).unwrap();
        len |= usize::from(b[0] & 127) << shift;
        if b[0] & 128 == 0 {
            break;
        }
        shift += 7;
    }
    let mut body = vec![0; len];
    s.read_exact(&mut body).unwrap();
    (head, body)
}
fn publish(s: &mut TcpStream, head: u8, b: &[u8]) -> (String, String) {
    let n = u16::from_be_bytes([b[0], b[1]]) as usize;
    let mut offset = 2 + n;
    if head & 6 == 2 {
        let _ = s.write_all(&[0x40, 2, b[offset], b[offset + 1]]);
        offset += 2;
    }
    (
        String::from_utf8(b[2..2 + n].to_vec()).unwrap(),
        String::from_utf8(b[offset..].to_vec()).unwrap(),
    )
}
#[test]
fn wire_connect_carries_retained_will_and_shutdown_publishes_offline() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let cfg = Config::parse(&format!(
        "[mqtt]\nhost='127.0.0.1'\nport={}\nusername='camera'\npassword='test-only'",
        listener.local_addr().unwrap().port()
    ))
    .unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = stop.clone();
    let worker = std::thread::spawn(move || {
        mqtt::run(
            cfg,
            FakeHardware,
            Arc::new(PipelineHealth::default()),
            &worker_stop,
        )
    });
    let (mut peer, _) = listener.accept().unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let (head, connect) = packet(&mut peer);
    assert_eq!(head, 0x10);
    assert_eq!(connect[7] & 0x26, 0x26, "retained QoS1 will");
    assert!(
        connect
            .windows(b"camera/e1zoom/availability".len())
            .any(|w| w == b"camera/e1zoom/availability")
    );
    assert!(connect.windows(7).any(|w| w == b"offline"));
    // A broker may take more than one polling interval to answer CONNECT.
    peer.set_read_timeout(Some(Duration::from_millis(350)))
        .unwrap();
    let wait = peer.read(&mut [0]);
    assert!(
        matches!(wait, Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut)),
        "CONNECT must stay alive until the network timeout"
    );
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    peer.write_all(&[0x20, 2, 0, 0]).unwrap();
    let mut discovery = 0;
    let mut temperature = false;
    let mut online = false;
    while !online || !temperature || discovery < 11 {
        let (h, b) = packet(&mut peer);
        match h >> 4 {
            3 => {
                let (topic, value) = publish(&mut peer, h, &b);
                if topic.starts_with("homeassistant/") {
                    discovery += 1;
                    assert_eq!(h & 1, 1);
                }
                if topic == "camera/e1zoom/availability" && value == "online" {
                    online = true;
                }
                if topic == "camera/e1zoom/temperature/state" {
                    assert_eq!(value, "56");
                    temperature = true;
                }
            }
            8 => {
                peer.write_all(&[0x90, 3, b[0], b[1], 1]).unwrap();
            }
            _ => {}
        }
    }
    // HA birth and a lost TCP connection both republish discovery without restarting the daemon.
    let topic = b"homeassistant/status";
    let mut birth = vec![0x30, (2 + topic.len() + 6) as u8, 0, topic.len() as u8];
    birth.extend_from_slice(topic);
    birth.extend_from_slice(b"online");
    peer.write_all(&birth).unwrap();
    let mut repeated = 0;
    while repeated < 11 {
        let (h, b) = packet(&mut peer);
        if h >> 4 == 3 && publish(&mut peer, h, &b).0.starts_with("homeassistant/") {
            repeated += 1;
        }
    }
    drop(peer);
    let (mut peer, _) = listener.accept().unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    assert_eq!(packet(&mut peer).0, 0x10);
    peer.write_all(&[0x20, 2, 0, 0]).unwrap();
    let mut repeated = 0;
    while repeated < 11 {
        let (h, b) = packet(&mut peer);
        match h >> 4 {
            3 if publish(&mut peer, h, &b).0.starts_with("homeassistant/") => repeated += 1,
            8 => peer.write_all(&[0x90, 3, b[0], b[1], 1]).unwrap(),
            _ => {}
        }
    }
    stop.store(true, Ordering::Relaxed);
    let mut offline = false;
    loop {
        let (h, b) = packet(&mut peer);
        if h >> 4 == 3 {
            let (t, v) = publish(&mut peer, h, &b);
            offline |= t == "camera/e1zoom/availability" && v == "offline";
        }
        if h >> 4 == 14 {
            break;
        }
    }
    worker.join().unwrap().unwrap();
    assert!(offline);
}

#[test]
fn hardware_worker_panic_reaches_the_owner_without_waiting_for_shutdown() {
    use std::sync::mpsc;
    struct PanickingHardware(mpsc::Sender<()>);
    impl Hardware for PanickingHardware {
        fn apply(&mut self, _: &Command) -> Result<(), String> {
            Ok(())
        }
        fn tick(&mut self, _: Duration) -> Result<Option<Duration>, String> {
            self.0.send(()).unwrap();
            panic!("injected ISP worker panic");
        }
        fn states(&mut self) -> Result<BTreeMap<String, String>, String> {
            Ok(BTreeMap::new())
        }
    }
    // Reserve an isolated port; no broker responds, so failure reporting cannot depend on MQTT.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let cfg = Config::parse(&format!(
        "[mqtt]\nhost='127.0.0.1'\nport={}",
        listener.local_addr().unwrap().port()
    ))
    .unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop);
    let (failed_tx, failed_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let result = std::panic::catch_unwind(|| {
            mqtt::run(
                cfg,
                PanickingHardware(failed_tx),
                Arc::new(PipelineHealth::default()),
                &worker_stop,
            )
        });
        done_tx.send(result).unwrap();
    });
    failed_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let result = done_rx.recv_timeout(Duration::from_secs(8));
    stop.store(true, Ordering::Relaxed);
    worker.join().unwrap();
    let result = result.expect("MQTT hid its dead hardware worker until shutdown");
    let error = result
        .expect("MQTT should return the worker failure")
        .unwrap_err();
    assert!(
        error.to_string().contains("hardware worker panicked"),
        "{error}"
    );
}

#[test]
fn stop_cancels_active_motion_discards_queued_motion_and_clears_retained_position() {
    use cam::{
        ir::{Ir, fake::FakeHw},
        mqtt::device::{Device, Platform},
        ptz::{Bus, Error, Ptz, fake::FakeBus},
        zoom::zf_table::ZfTable,
        zoom::{Lens, fake::FakeIsp},
    };
    use std::sync::{Mutex, mpsc};
    struct Os;
    impl Platform for Os {
        fn reboot(&mut self) -> Result<(), String> {
            Ok(())
        }
    }
    struct SlowBus {
        bus: FakeBus,
        armed: Arc<AtomicBool>,
        started: mpsc::SyncSender<()>,
        moving: bool,
    }
    impl Bus for SlowBus {
        fn read(&mut self, reg: u8, out: &mut [u8]) -> Result<(), Error> {
            if self.moving && matches!(reg, 0x8b | 0x8d) {
                out[0] = 0x90;
                return Ok(());
            }
            self.bus.read(reg, out)
        }
        fn write(&mut self, reg: u8, data: &[u8]) -> Result<(), Error> {
            if reg == 9 && data[0] & 0xc0 != 0 && self.armed.load(Ordering::Acquire) {
                self.moving = true;
            }
            if reg == 0 && data[0] & 6 == 6 {
                self.moving = false;
            }
            self.bus.write(reg, data)
        }
        fn sleep(&mut self, d: Duration) {
            self.bus.sleep(d);
            if self.armed.load(Ordering::Acquire) && d == Duration::from_millis(10) {
                let _ = self.started.try_send(());
                std::thread::sleep(d);
            }
        }
    }
    struct Observed<H> {
        hardware: H,
        applied: Arc<Mutex<Vec<Command>>>,
    }
    impl<H: Hardware> Hardware for Observed<H> {
        fn stop_endpoint(&self) -> Option<std::path::PathBuf> {
            self.hardware.stop_endpoint()
        }
        fn motion_epoch(&self) -> Option<Arc<std::sync::atomic::AtomicU32>> {
            self.hardware.motion_epoch()
        }
        fn apply(&mut self, c: &Command) -> Result<(), String> {
            self.applied.lock().unwrap().push(c.clone());
            self.hardware.apply(c)
        }
        fn tick(&mut self, t: Duration) -> Result<Option<Duration>, String> {
            self.hardware.tick(t)
        }
        fn states(&mut self) -> Result<BTreeMap<String, String>, String> {
            self.hardware.states()
        }
    }
    fn command(peer: &mut TcpStream, id: &str, value: &str) {
        let topic = format!("camera/e1zoom/{id}/set");
        let mut bytes = vec![
            0x30,
            (2 + topic.len() + value.len()) as u8,
            0,
            topic.len() as u8,
        ];
        bytes.extend_from_slice(topic.as_bytes());
        bytes.extend_from_slice(value.as_bytes());
        peer.write_all(&bytes).unwrap();
    }
    /// Waits for `expected` on the state topic of `id`, or finds it among the publications `seen`
    /// earlier (states are published once per change, so one may pass while waiting for another).
    /// Keep-alive pings keep the socket busy; the deadline turns a missing state into a failure.
    fn state(peer: &mut TcpStream, seen: &mut Vec<(String, String)>, id: &str, expected: &str) {
        let wanted = (format!("camera/e1zoom/{id}/state"), expected.to_owned());
        if id != "stop"
            && let Some(at) = seen.iter().position(|p| *p == wanted)
        {
            seen.remove(at);
            return;
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            assert!(Instant::now() < deadline, "no {wanted:?}; seen {seen:?}");
            let (h, b) = packet(peer);
            match h >> 4 {
                3 => {
                    let (topic, value) = publish(peer, h, &b);
                    if (topic.as_str(), value.as_str()) == (wanted.0.as_str(), expected) {
                        if id == "stop" {
                            assert_eq!(
                                h & 1,
                                0,
                                "stop acknowledgment must not be replayed as retained state"
                            );
                        }
                        return;
                    }
                    seen.push((topic, value));
                }
                8 => peer.write_all(&[0x90, 3, b[0], b[1], 1]).unwrap(),
                12 => peer.write_all(&[0xd0, 0]).unwrap(),
                _ => {}
            }
        }
    }
    let root = std::env::temp_dir().join(format!("cam-mqtt-stop-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let owner = cam::control_stop::Owner::bind(&root.join("stop")).unwrap();
    let armed = Arc::new(AtomicBool::new(false));
    let (started_tx, started_rx) = mpsc::sync_channel(1);
    let hardware = Device::new(
        Ptz::new(SlowBus {
            bus: FakeBus::new(3000, 500),
            armed: armed.clone(),
            started: started_tx,
            moving: false,
        })
        .with_stop_owner(owner),
        Lens::new(FakeIsp::new(0, 230)),
        Ir::new(FakeHw::new(1800)),
        Os,
        ZfTable::parse(b"<ZFTable>1000000,e6,400,3fe</ZFTable>").unwrap(),
        cam::settings::Owner::load(
            Box::new(cam::settings::FakeDisk::default()),
            "",
            Box::new(cam::settings::FakeDisk::default()),
            Default::default(),
            Box::new(cam::settings::FakeDisk::default()),
        ),
    )
    .unwrap();
    let applied = Arc::new(Mutex::new(Vec::new()));
    let hardware = Observed {
        hardware,
        applied: applied.clone(),
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let cfg = Config::parse(&format!(
        "[mqtt]\nhost='127.0.0.1'\nport={}",
        listener.local_addr().unwrap().port()
    ))
    .unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = stop.clone();
    let worker = std::thread::spawn(move || {
        mqtt::run(
            cfg,
            hardware,
            Arc::new(PipelineHealth::default()),
            &worker_stop,
        )
    });
    let (mut peer, _) = listener.accept().unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    assert_eq!(packet(&mut peer).0, 0x10);
    peer.write_all(&[0x20, 2, 0, 0]).unwrap();
    let mut seen = Vec::new();
    state(&mut peer, &mut seen, "pan", "0");
    armed.store(true, Ordering::Release);
    command(&mut peer, "pan", "7000");
    started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    command(&mut peer, "pan", "1234");
    command(&mut peer, "tilt", "456");
    command(&mut peer, "stop", "PRESS");
    // The hardware thread may clear the position before the stop acknowledgment goes out.
    state(&mut peer, &mut seen, "stop", "stopped");
    // A later non-motion command is a barrier beyond all discarded queued motion.
    command(&mut peer, "led", "OFF");
    state(&mut peer, &mut seen, "led", "OFF");
    assert_eq!(
        *applied.lock().unwrap(),
        vec![Command::Pan(7000), Command::Setting("led", "OFF".into())]
    );
    // After the stop, the unknown coordinates are retained tombstones (published once, on change).
    state(&mut peer, &mut seen, "pan", "");
    state(&mut peer, &mut seen, "tilt", "");
    stop.store(true, Ordering::Release);
    loop {
        let (h, b) = packet(&mut peer);
        if h >> 4 == 3 {
            publish(&mut peer, h, &b);
        }
        if h >> 4 == 14 {
            break;
        }
    }
    worker.join().unwrap().unwrap();
    assert!(!root.join("stop").exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn terminal_hardware_fault_exits_the_joined_worker_instead_of_reporting_progress() {
    struct Fault {
        failed: bool,
    }
    impl Hardware for Fault {
        fn failed(&self) -> bool {
            self.failed
        }
        fn apply(&mut self, _: &Command) -> Result<(), String> {
            Ok(())
        }
        fn tick(&mut self, _: Duration) -> Result<Option<Duration>, String> {
            self.failed = true;
            Err("coil release failed".into())
        }
        fn states(&mut self) -> Result<BTreeMap<String, String>, String> {
            panic!("terminal hardware cannot publish confirmed state")
        }
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let cfg = Config::parse(&format!(
        "[mqtt]\nhost='127.0.0.1'\nport={}",
        listener.local_addr().unwrap().port()
    ))
    .unwrap();
    let stop = AtomicBool::new(false);
    let result = mqtt::run(
        cfg,
        Fault { failed: false },
        Arc::new(PipelineHealth::default()),
        &stop,
    );
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("hardware worker exited unexpectedly")
    );
    assert!(stop.load(Ordering::Acquire));
}

#[test]
fn broker_cannot_resume_a_clean_session_or_leave_hardware_running() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let cfg = Config::parse(&format!(
        "[mqtt]\nhost='127.0.0.1'\nport={}",
        listener.local_addr().unwrap().port()
    ))
    .unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let ws = Arc::clone(&stop);
    let worker = std::thread::spawn(move || {
        mqtt::run(cfg, FakeHardware, Arc::new(PipelineHealth::default()), &ws)
    });
    let (mut peer, _) = listener.accept().unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let (_, connect) = packet(&mut peer);
    assert_ne!(connect[7] & 2, 0);
    peer.write_all(&[0x20, 2, 1, 0]).unwrap();
    assert!(
        worker
            .join()
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("clean MQTT session")
    );
    assert!(stop.load(Ordering::Acquire));
}

#[test]
fn a_preset_saved_through_the_shared_motion_queue_updates_the_select_live() {
    // The hardware owns the preset table; ONVIF's path (Motion) saves into it.
    struct Table(Vec<String>);
    impl Hardware for Table {
        fn apply(&mut self, c: &Command) -> Result<(), String> {
            match c {
                Command::SavePreset(name) => self.0.push(name.clone()),
                Command::RemovePreset(name) => self.0.retain(|n| n != name),
                _ => {}
            }
            Ok(())
        }
        fn tick(&mut self, _: Duration) -> Result<Option<Duration>, String> {
            Ok(None)
        }
        fn states(&mut self) -> Result<BTreeMap<String, String>, String> {
            Ok(BTreeMap::new())
        }
        fn preset_names(&self) -> Option<Vec<String>> {
            Some(self.0.clone())
        }
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let cfg = Config::parse(&format!(
        "[mqtt]\nhost='127.0.0.1'\nport={}",
        listener.local_addr().unwrap().port()
    ))
    .unwrap();
    let motion = Arc::new(mqtt::Motion::default());
    let stop = Arc::new(AtomicBool::new(false));
    let (worker_stop, worker_motion) = (stop.clone(), motion.clone());
    let worker = std::thread::spawn(move || {
        mqtt::run_full(
            cfg,
            Table(Vec::new()),
            Arc::new(PipelineHealth::default()),
            &worker_stop,
            None,
            None,
            Some(worker_motion),
        )
    });
    let (mut peer, _) = listener.accept().unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    assert_eq!(packet(&mut peer).0, 0x10);
    peer.write_all(&[0x20, 2, 0, 0]).unwrap();
    let is_select =
        |t: &str| t.starts_with("homeassistant/select/") && t.ends_with("/preset/config");
    let select = |peer: &mut TcpStream| loop {
        let (h, b) = packet(peer);
        match h >> 4 {
            3 => {
                let (topic, value) = publish(peer, h, &b);
                if is_select(&topic) {
                    return value;
                }
            }
            8 => peer.write_all(&[0x90, 3, b[0], b[1], 1]).unwrap(),
            _ => {}
        }
    };
    assert_eq!(select(&mut peer), "", "no presets: the select is cleared");
    assert!(motion.presets().is_empty());
    motion.submit(Command::SavePreset("Window".into())).unwrap();
    let options: serde_json::Value = serde_json::from_str(&select(&mut peer)).unwrap();
    assert_eq!(options["options"], serde_json::json!(["Window"]));
    assert_eq!(motion.presets(), ["Window"]);
    motion
        .submit(Command::RemovePreset("Window".into()))
        .unwrap();
    assert_eq!(
        select(&mut peer),
        "",
        "the last preset removed clears it again"
    );
    assert!(motion.presets().is_empty());
    stop.store(true, Ordering::Relaxed);
    drop(peer);
    worker.join().unwrap().unwrap();
}
