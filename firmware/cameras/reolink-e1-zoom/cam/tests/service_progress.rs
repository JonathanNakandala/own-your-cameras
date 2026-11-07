//! Each real service reports progress only from its own completed loop iterations.
use cam::mqtt::{self, Command, Config, Hardware, PipelineHealth};
use cam::net::{Host, Proc, Timing};
use cam::rtsp::hub::Hub;
use cam::rtsp::server::{self, Config as RtspConfig};
use cam::runtime::production::net_service;
use cam::runtime::{Beats, LifecycleHealth, Monotonic, Progress};
use std::collections::BTreeMap;
use std::io;
use std::net::{Ipv4Addr, TcpListener};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn leak_flag() -> &'static AtomicBool {
    Box::leak(Box::new(AtomicBool::new(false)))
}

fn until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn beats() -> (LifecycleHealth, Beats) {
    let health = LifecycleHealth::new();
    (health.clone(), Beats::new(health, Monotonic::default()))
}

#[test]
fn rtsp_beats_every_idle_loop_and_nothing_else() {
    let (health, beats) = beats();
    let stop = leak_flag();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let cfg = Arc::new(RtspConfig {
        streams: vec![camera_transport::StreamPath::new("main", Hub::new())],
        creds: None,
        talk: None,
    });
    let worker = std::thread::spawn(move || server::serve_with(&listener, cfg, stop, Some(&beats)));
    until("rtsp beat", || health.last_beat(Progress::Rtsp).is_some());
    for other in [
        Progress::MainEncoder,
        Progress::SubEncoder,
        Progress::Network,
        Progress::Mqtt,
        Progress::Hardware,
    ] {
        assert_eq!(health.last_beat(other), None, "{other:?} must not be faked");
    }
    stop.store(true, Ordering::Relaxed);
    worker.join().unwrap().unwrap();
}

struct IdleHardware {
    hold: Arc<Mutex<()>>,
}
impl Hardware for IdleHardware {
    fn apply(&mut self, _: &Command) -> Result<(), String> {
        Ok(())
    }
    fn tick(&mut self, _: Duration) -> Result<Option<Duration>, String> {
        // A blocked motor/ISP call keeps the hardware loop from completing an iteration.
        drop(self.hold.lock().unwrap());
        Ok(None)
    }
    fn states(&mut self) -> Result<BTreeMap<String, String>, String> {
        Ok(BTreeMap::new())
    }
}

#[test]
fn mqtt_and_hardware_beat_independently_even_with_the_broker_down() {
    let (health, beats) = beats();
    let stop = leak_flag();
    let hold = Arc::new(Mutex::new(()));
    let blocked = hold.lock().unwrap();
    // Nothing listens here: the network loop only ever sees connection failures.
    let closed = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);
    let cfg = Config::parse(&format!("[mqtt]\nhost='127.0.0.1'\nport={port}")).unwrap();
    let worker_beats = beats.clone();
    let worker_hold = Arc::clone(&hold);
    let worker = std::thread::spawn(move || {
        mqtt::run_with(
            cfg,
            IdleHardware { hold: worker_hold },
            Arc::new(PipelineHealth::default()),
            stop,
            Some(&worker_beats),
        )
    });
    until("mqtt reconnect-loop beat", || {
        health.last_beat(Progress::Mqtt).is_some()
    });
    let stalled = health.last_beat(Progress::Hardware);
    std::thread::sleep(Duration::from_millis(1200));
    assert_eq!(
        health.last_beat(Progress::Hardware),
        stalled,
        "a blocked hardware call must not be covered by the MQTT loop"
    );
    drop(blocked);
    until("hardware beat after the call returns", || {
        health.last_beat(Progress::Hardware) != stalled
    });
    stop.store(true, Ordering::Relaxed);
    worker.join().unwrap().unwrap();
}

#[derive(Default)]
struct NetState {
    ticks: usize,
    panic_on_power: bool,
    dropped: bool,
}
struct FakeNet(Arc<Mutex<NetState>>);
impl Drop for FakeNet {
    fn drop(&mut self) {
        self.0.lock().unwrap().dropped = true;
    }
}
impl Host for FakeNet {
    fn power_wifi(&mut self) -> io::Result<()> {
        let mut s = self.0.lock().unwrap();
        s.ticks += 1;
        let panic_on_power = s.panic_on_power;
        drop(s);
        assert!(!panic_on_power, "network adapter unwound");
        Ok(())
    }
    fn module_loaded(&self, _: &str) -> io::Result<bool> {
        Ok(true)
    }
    fn load_wifi_modules(&mut self) -> io::Result<()> {
        // A load that fails every time is a recoverable fault: the loop still completes.
        Err(io::Error::other("insmod failed"))
    }
    fn unload_wifi_modules(&mut self) -> io::Result<()> {
        Ok(())
    }
    fn load_eth_module(&mut self) -> io::Result<()> {
        Ok(())
    }
    fn iface_exists(&self, _: &str) -> bool {
        false
    }
    fn iface_is_up(&self, _: &str) -> bool {
        false
    }
    fn iface_up(&mut self, _: &str) -> io::Result<()> {
        Ok(())
    }
    fn carrier(&self, _: &str) -> bool {
        false
    }
    fn ipv4(&self, _: &str) -> Option<Ipv4Addr> {
        None
    }
    fn alive(&mut self, _: Proc) -> io::Result<bool> {
        Ok(false)
    }
    fn spawn(&mut self, _: Proc) -> io::Result<()> {
        Ok(())
    }
    fn kill(&mut self, _: Proc) {}
}

#[test]
fn network_beats_through_retryable_faults_and_returns_the_host_on_stop() {
    let (health, beats) = beats();
    let stop = leak_flag();
    let state = Arc::new(Mutex::new(NetState::default()));
    let slot = Arc::new(Mutex::new(Some(FakeNet(Arc::clone(&state)))));
    let service = net_service(
        Arc::clone(&slot),
        Timing::default(),
        Duration::from_millis(20),
    );
    let worker = std::thread::spawn(move || service(beats, stop));
    until("network beat", || {
        health.last_beat(Progress::Network).is_some()
    });
    let first = health.last_beat(Progress::Network);
    until("a second completed loop", || {
        health.last_beat(Progress::Network) != first
    });
    assert!(state.lock().unwrap().ticks >= 1);
    assert!(
        slot.lock().unwrap().is_none(),
        "the worker owns the host while it runs"
    );
    stop.store(true, Ordering::Relaxed);
    worker.join().unwrap().unwrap();
    assert!(
        slot.lock().unwrap().is_some(),
        "host handed back for exit_preserving_network"
    );
}

#[test]
fn network_failure_preserves_the_host_for_terminal_recovery() {
    let (health, beats) = beats();
    let stop = leak_flag();
    let state = Arc::new(Mutex::new(NetState {
        panic_on_power: true,
        ..NetState::default()
    }));
    let slot = Arc::new(Mutex::new(Some(FakeNet(Arc::clone(&state)))));
    let service = net_service(Arc::clone(&slot), Timing::default(), Duration::ZERO);
    let worker = std::thread::spawn(move || service(beats, stop));
    assert!(worker.join().is_err(), "exercise the real service unwind");
    assert_eq!(health.last_beat(Progress::Network), None);
    assert!(
        !state.lock().unwrap().dropped,
        "dropping the network host kills the owned DHCP/supplicant children"
    );
    assert!(slot.lock().unwrap().is_some(), "owner retained for exit");
    drop(slot);
    assert!(state.lock().unwrap().dropped, "fixture owner is cleaned up");
}

#[test]
fn mqtt_network_unwind_stops_nested_hardware_before_the_scope_joins() {
    use cam::runtime::Clock;
    #[derive(Clone)]
    struct FaultClock;
    impl Clock for FaultClock {
        fn now(&self) -> Duration {
            assert_ne!(
                std::thread::current().name(),
                Some("mqtt-run"),
                "clock fault in network loop"
            );
            Duration::ZERO
        }
        fn wait(&mut self, _: Duration) {}
    }
    let stop = leak_flag();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let cfg = Config::parse(&format!(
        "[mqtt]\nhost='127.0.0.1'\nport={}",
        listener.local_addr().unwrap().port()
    ))
    .unwrap();
    let beats = Beats::new(LifecycleHealth::new(), FaultClock);
    let worker = std::thread::Builder::new()
        .name("mqtt-run".into())
        .spawn(move || {
            mqtt::run_with(
                cfg,
                IdleHardware {
                    hold: Arc::default(),
                },
                Arc::new(PipelineHealth::default()),
                stop,
                Some(&beats),
            )
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !worker.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    let finished = worker.is_finished();
    // Rescue the pre-fix scope so a failed regression never leaks its threads.
    stop.store(true, Ordering::Release);
    assert!(worker.join().is_err());
    assert!(
        finished,
        "MQTT scope joined live hardware without first requesting stop"
    );
}
