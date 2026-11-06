//! The production orchestration (`cam::runtime::production::run`) driven at its public seams: the watchdog
//! handoff/device, the HDAL backend, the clock, and the three service workers.
use cam::mqtt::PipelineHealth;
use cam::pipeline::PipelineConfig;
use cam::rtsp::{audio::AudioHub, hub::VideoHubs, hub::Viewer};
use cam::runtime::production::{Outcome, Service, Services, Wiring, run};
use cam::runtime::{Clock, Handoff, HealthObserver, Monotonic, Progress};
use cam::wdt::WatchdogDevice;
use hdal::Step;
use hdal::fake::{Control, Fake};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::*};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

type Trace = Arc<Mutex<Vec<&'static str>>>;
type Flag = Arc<AtomicBool>;

fn flag(on: bool) -> Flag {
    Arc::new(AtomicBool::new(on))
}
fn leak_flag() -> &'static AtomicBool {
    Box::leak(Box::new(AtomicBool::new(false)))
}
fn nap() {
    std::thread::sleep(Duration::from_millis(1));
}
fn until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        nap();
    }
}

/// Deterministic attempt clock: time only moves when the test says so.
#[derive(Clone, Default)]
struct ManualClock {
    ms: Arc<AtomicU64>,
    waits: Arc<AtomicUsize>,
    closed: Flag,
    panic_network: Flag,
}
impl Clock for ManualClock {
    fn now(&self) -> Duration {
        assert!(
            !(self.panic_network.load(Acquire) && std::thread::current().name() == Some("network")),
            "network clock fault"
        );
        Duration::from_millis(self.ms.load(Acquire))
    }
    fn wait(&mut self, d: Duration) {
        self.waits.fetch_add(1, AcqRel);
        let target = self.now() + d;
        while self.now() < target && !self.closed.load(Acquire) {
            nap();
        }
    }
}
impl ManualClock {
    /// One second of attempt time; returns once the feeder consumed it (or is gone).
    fn tick(&self) {
        let waits = self.waits.load(Acquire);
        self.ms.fetch_add(1000, AcqRel);
        let deadline = Instant::now() + Duration::from_millis(150);
        while self.waits.load(Acquire) == waits && Instant::now() < deadline {
            nap();
        }
        // Give every service a chance to re-stamp at the new time.
        std::thread::sleep(Duration::from_millis(8));
    }
    fn secs(&self) -> u64 {
        self.ms.load(Acquire) / 1000
    }
}

struct Dev {
    feeds: Arc<AtomicUsize>,
    trace: Trace,
    fail_keepalive_at: Option<usize>,
    fail_arm: bool,
    stop_on_arm: Option<&'static AtomicBool>,
    stop_on_feed_error: Option<&'static AtomicBool>,
}
impl WatchdogDevice for Dev {
    fn set_timeout(&mut self, secs: u32) -> io::Result<u32> {
        assert_eq!(secs, 60);
        self.trace.lock().unwrap().push("wdt_arm");
        if let Some(stop) = self.stop_on_arm {
            stop.store(true, Release);
        }
        if self.fail_arm {
            return Err(io::Error::other("arm"));
        }
        Ok(secs)
    }
    fn keepalive(&mut self) -> io::Result<()> {
        let n = self.feeds.fetch_add(1, AcqRel) + 1;
        if self.fail_keepalive_at == Some(n) {
            if let Some(stop) = self.stop_on_feed_error {
                stop.store(true, Release);
            }
            return Err(io::Error::other("keepalive"));
        }
        Ok(())
    }
    fn disarm(&mut self) -> io::Result<()> {
        self.trace.lock().unwrap().push("DISARM");
        Ok(())
    }
}
struct Hand {
    dev: Option<Dev>,
    open_error: Option<i32>,
    panic_open: bool,
    stop_on_open: Option<&'static AtomicBool>,
    on_open: Option<Box<dyn FnOnce() + Send>>,
    lock: Option<cam::runtime::s00::S00Handoff>,
    trace: Trace,
}
impl Handoff for Hand {
    type Device = Dev;
    fn request_release(&mut self) -> io::Result<()> {
        self.trace.lock().unwrap().push("wdt_release_requested");
        Ok(())
    }
    fn open(&mut self) -> io::Result<Dev> {
        if let Some(action) = self.on_open.take() {
            action();
        }
        assert!(!self.panic_open, "watchdog open panicked");
        if let Some(stop) = self.stop_on_open {
            stop.store(true, Release);
        }
        if let Some(code) = self.open_error {
            return Err(io::Error::from_raw_os_error(code));
        }
        Ok(self.dev.take().expect("opened once"))
    }
}

struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "cam-production-{}-{}",
            std::process::id(),
            N.fetch_add(1, Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn health(&self) -> Option<String> {
        std::fs::read_to_string(self.0.join("cam-health-boot1-a1.json")).ok()
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Pauses are per beat source: while set, the fake service stops beating that source (a stuck loop).
#[derive(Clone)]
struct Pauses {
    net: Flag,
    rtsp: Flag,
    mqtt: Flag,
    hw: Flag,
}
impl Pauses {
    fn new() -> Self {
        Self {
            net: flag(false),
            rtsp: flag(false),
            mqtt: flag(false),
            hw: flag(false),
        }
    }
}

/// Per source, the attempt time (ms) of its latest completed beat; `u64::MAX` before the first.
type Beaten = Arc<[AtomicU64; 6]>;

fn beating(
    parts: Vec<(Progress, Flag)>,
    beaten: &Beaten,
    trace: &Trace,
    started: &'static str,
    exited: &'static str,
    hold_exit: Option<Flag>,
) -> Service {
    let trace = Arc::clone(trace);
    let beaten = Arc::clone(beaten);
    Box::new(move |beats, stop| {
        trace.lock().unwrap().push(started);
        while !stop.load(Relaxed) {
            for (source, paused) in &parts {
                if !paused.load(Relaxed) {
                    let at = beats.now();
                    beats.beat(*source);
                    let at = u64::try_from(at.as_millis()).unwrap();
                    beaten[*source as usize].store(at, Release);
                }
            }
            nap();
        }
        if let Some(hold) = hold_exit {
            while hold.load(Acquire) {
                nap();
            }
        }
        trace.lock().unwrap().push(exited);
        Ok(())
    })
}

struct Rig {
    clock: ManualClock,
    stop: &'static AtomicBool,
    feeds: Arc<AtomicUsize>,
    trace: Trace,
    control: Arc<Control>,
    pauses: Pauses,
    dir: TempDir,
    hold_join: Flag,
    beaten: Beaten,
    hubs: VideoHubs,
}

#[derive(Default)]
struct Options {
    hdal: Option<Fake>,
    open_error: Option<i32>,
    fail_arm: bool,
    fail_keepalive_at: Option<usize>,
    network: Option<Service>,
    rtsp: Option<Service>,
    mqtt: Option<Service>,
}
impl Rig {
    fn new() -> Self {
        Self {
            clock: ManualClock::default(),
            stop: leak_flag(),
            feeds: Arc::default(),
            trace: Arc::default(),
            control: Arc::default(),
            pauses: Pauses::new(),
            dir: TempDir::new(),
            hold_join: flag(false),
            beaten: Arc::new(std::array::from_fn(|_| AtomicU64::new(u64::MAX))),
            hubs: VideoHubs::new(),
        }
    }

    fn wiring<C: Clock + Clone + Send + Sync + 'static>(
        &self,
        clock: C,
        o: Options,
    ) -> Wiring<Hand, Fake, C> {
        let mut hdal = o.hdal.unwrap_or_default();
        hdal.trace = Some(Arc::clone(&self.trace));
        hdal.control = Some(Arc::clone(&self.control));
        let t = &self.trace;
        let p = &self.pauses;
        Wiring {
            handoff: Hand {
                dev: Some(Dev {
                    feeds: Arc::clone(&self.feeds),
                    trace: Arc::clone(t),
                    fail_keepalive_at: o.fail_keepalive_at,
                    fail_arm: o.fail_arm,
                    stop_on_arm: None,
                    stop_on_feed_error: None,
                }),
                open_error: o.open_error,
                panic_open: false,
                stop_on_open: None,
                on_open: None,
                lock: None,
                trace: Arc::clone(t),
            },
            clock,
            health_dir: self.dir.0.clone(),
            attempt: "boot1-a1".into(),
            pid: std::process::id(),
            stop: self.stop,
            hdal,
            media: PipelineConfig::e1_zoom(),
            hubs: VideoHubs {
                main: Arc::clone(&self.hubs.main),
                sub: Arc::clone(&self.hubs.sub),
            },
            audio: AudioHub::new(),
            talk: cam::rtsp::opus::talkback(hdal::audio::RATE),
            pipeline: Arc::new(PipelineHealth::default()),
            media_tuning: Default::default(),
            counters: Default::default(),
            services: Services {
                network: o.network.unwrap_or_else(|| {
                    beating(
                        vec![(Progress::Network, Arc::clone(&p.net))],
                        &self.beaten,
                        t,
                        "network_started",
                        "network_exited",
                        None,
                    )
                }),
                rtsp: o.rtsp.unwrap_or_else(|| {
                    beating(
                        vec![(Progress::Rtsp, Arc::clone(&p.rtsp))],
                        &self.beaten,
                        t,
                        "rtsp_started",
                        "rtsp_exited",
                        None,
                    )
                }),
                mqtt: o.mqtt.unwrap_or_else(|| {
                    beating(
                        vec![
                            (Progress::Mqtt, Arc::clone(&p.mqtt)),
                            (Progress::Hardware, Arc::clone(&p.hw)),
                        ],
                        &self.beaten,
                        t,
                        "mqtt_started",
                        "mqtt_exited",
                        Some(Arc::clone(&self.hold_join)),
                    )
                }),
            },
        }
    }

    fn start(&self, o: Options) -> JoinHandle<Outcome<Hand>> {
        let wiring = self.wiring(self.clock.clone(), o);
        std::thread::spawn(move || run(wiring))
    }

    fn at(&self, event: &str) -> Option<usize> {
        self.trace.lock().unwrap().iter().position(|e| *e == event)
    }
    fn saw(&self, event: &str) -> bool {
        self.at(event).is_some()
    }
    fn control(&self) -> &Control {
        &self.control
    }

    /// Viewers of both encoder streams, for [`Rig::step`]; subscribe before the run starts.
    fn video(&self) -> [Viewer; 2] {
        [self.hubs.main.subscribe(), self.hubs.sub.subscribe()]
    }

    /// One attempt second in lockstep, waiting on state instead of real time: every beat source has
    /// beaten at the current second (so none can look stale under host load), then the clock moves
    /// and the feeder has polled and published at the new second. Only for an unpaused, live run.
    fn step(&self, video: &[Viewer; 2]) {
        let now = self.clock.ms.load(Acquire);
        let sources = [
            Progress::Network,
            Progress::Rtsp,
            Progress::Mqtt,
            Progress::Hardware,
        ];
        until("service beats", || {
            sources.iter().all(|s| {
                let at = self.beaten[*s as usize].load(Acquire);
                at != u64::MAX && at >= now
            })
        });
        // A frame received after the queue drained was published after it, so the frame after that
        // was beaten (beat before publish, one thread) at the current second.
        for viewer in video {
            while viewer.try_next().unwrap().is_some() {}
        }
        let mut frames = [0; 2];
        until("encoder beats", || {
            for (n, viewer) in frames.iter_mut().zip(video) {
                *n += usize::from(viewer.try_next().unwrap().is_some());
            }
            frames.iter().all(|n| *n >= 2)
        });
        let waits = self.clock.waits.load(Acquire);
        self.clock.ms.fetch_add(1000, AcqRel);
        until("feeder poll", || self.clock.waits.load(Acquire) != waits);
    }

    /// Advance whole seconds until the run ends (or `limit` seconds pass); returns the clock second.
    fn drive_until_done(&self, run: &JoinHandle<Outcome<Hand>>, limit: u64) -> u64 {
        while !run.is_finished() && self.clock.secs() < limit {
            self.clock.tick();
        }
        self.clock.secs()
    }
}
impl Drop for Rig {
    fn drop(&mut self) {
        self.stop.store(true, Relaxed);
        self.hold_join.store(false, Relaxed);
        self.control.block_recv.store(false, Relaxed);
        *self.control.hold_step.lock().unwrap() = None;
        self.clock.closed.store(true, Relaxed);
    }
}

fn finish(run: JoinHandle<Outcome<Hand>>) -> Outcome<Hand> {
    until("run to finish", || run.is_finished());
    run.join().unwrap()
}

fn err(outcome: &Outcome<Hand>) -> String {
    outcome
        .result
        .as_ref()
        .expect_err("run must fail")
        .to_string()
}

#[test]
fn healthy_run_arms_before_hdal_publishes_health_and_never_disarms() {
    let rig = Rig::new();
    let run = rig.start(Options::default());
    until("services running", || rig.saw("rtsp_started"));
    let (arm, init, mqtt, rtsp) = (
        rig.at("wdt_arm").unwrap(),
        rig.at("common_init").unwrap(),
        rig.at("mqtt_started").unwrap(),
        rig.at("rtsp_started").unwrap(),
    );
    assert!(rig.at("wdt_release_requested").unwrap() < arm);
    assert!(
        arm < init,
        "no HDAL before the watchdog is armed and publishing"
    );
    assert!(
        init < mqtt && init < rtsp,
        "services start only after HDAL is up"
    );
    until("network worker", || rig.saw("network_started"));
    assert!(rig.feeds.load(Acquire) >= 1);
    let record = rig.dir.health().expect("current-attempt health file");
    assert!(record.contains("\"attempt\":\"boot1-a1\""), "{record}");
    rig.stop.store(true, Relaxed);
    rig.drive_until_done(&run, 100);
    let outcome = finish(run);
    assert!(outcome.result.is_ok(), "{:?}", outcome.result.err());
    assert!(
        outcome.handoff.is_some(),
        "runtime lock handed back to the caller"
    );
    assert!(!rig.saw("DISARM"));
    assert!(rig.dir.health().is_none(), "health invalidated on exit");
    // Everything joined before HDAL teardown.
    let joined = rig
        .at("mqtt_exited")
        .unwrap()
        .max(rig.at("rtsp_exited").unwrap());
    assert!(joined < rig.at("videocap_stop").unwrap());
    assert!(joined < rig.at("common_uninit").unwrap());
}

#[test]
fn nothing_starts_when_the_watchdog_cannot_be_acquired() {
    for case in 0..3 {
        let rig = Rig::new();
        let options = match case {
            0 => Options {
                open_error: Some(libc::EACCES),
                ..Options::default()
            },
            1 => Options {
                fail_arm: true,
                ..Options::default()
            },
            _ => Options {
                fail_keepalive_at: Some(1),
                ..Options::default()
            },
        };
        let outcome = finish(rig.start(options));
        assert!(
            err(&outcome).contains("watchdog"),
            "case {case}: {}",
            err(&outcome)
        );
        for never in [
            "common_init",
            "network_started",
            "mqtt_started",
            "rtsp_started",
        ] {
            assert!(
                !rig.saw(never),
                "case {case}: {never} started without a watchdog"
            );
        }
        assert!(!rig.saw("DISARM"));
        assert!(rig.dir.health().is_none());
        assert!(outcome.handoff.is_some());
    }
}

#[test]
fn an_unwritable_health_directory_fails_before_the_handoff() {
    let rig = Rig::new();
    let mut wiring = rig.wiring(Monotonic::default(), Options::default());
    wiring.health_dir = rig.dir.0.join("missing/dir");
    let outcome = run(wiring);
    assert!(outcome.result.is_err());
    assert!(
        !rig.saw("wdt_release_requested"),
        "S00 must not be released"
    );
    assert!(!rig.saw("common_init"));
}

#[test]
fn an_invalid_attempt_token_is_refused_before_anything_happens() {
    let rig = Rig::new();
    let mut wiring = rig.wiring(Monotonic::default(), Options::default());
    wiring.attempt = "../escape".into();
    assert!(run(wiring).result.is_err());
    assert!(!rig.saw("wdt_release_requested"));
}

#[test]
fn a_failing_media_graph_never_starts_services_and_stops_feeding() {
    let rig = Rig::new();
    let run = rig.start(Options {
        hdal: Some(Fake::failing(Step::CommonMemInit, -7)),
        ..Options::default()
    });
    rig.drive_until_done(&run, 100);
    let outcome = finish(run);
    assert!(err(&outcome).contains("media"), "{}", err(&outcome));
    assert!(!rig.saw("mqtt_started") && !rig.saw("rtsp_started"));
    assert!(rig.saw("network_exited"), "network worker joined");
    assert!(!rig.saw("DISARM"));
}

#[test]
fn worker_return_error_and_unwind_end_the_boot_even_during_grace() {
    for (which, mode) in (0..3).flat_map(|w| (0..3).map(move |m| (w, m))) {
        let rig = Rig::new();
        let bad = |t: &Trace| -> Service {
            let t = Arc::clone(t);
            Box::new(move |_, _| {
                t.lock().unwrap().push("bad_worker_ran");
                match mode {
                    0 => Ok(()),
                    1 => Err(io::Error::other("worker failed")),
                    _ => panic!("worker unwound"),
                }
            })
        };
        let mut options = Options::default();
        match which {
            0 => options.network = Some(bad(&rig.trace)),
            1 => options.rtsp = Some(bad(&rig.trace)),
            _ => options.mqtt = Some(bad(&rig.trace)),
        }
        // Real clock: far inside the 90 s startup grace, so only the lease can stop the run.
        let wiring = rig.wiring(Monotonic::default(), options);
        let run = std::thread::spawn(move || run(wiring));
        let outcome = finish(run);
        let message = err(&outcome);
        assert!(
            message.contains(["network", "rtsp", "mqtt"][which]),
            "worker {which} mode {mode}: {message}"
        );
        assert!(rig.saw("bad_worker_ran"));
        if which == 0 {
            // Nothing else may keep feeding for a dead required worker.
            assert!(
                rig.feeds.load(Acquire) <= 2,
                "fed {} times",
                rig.feeds.load(Acquire)
            );
        }
        assert!(!rig.saw("DISARM"));
        assert!(rig.dir.health().is_none());
    }
}

#[test]
fn a_stalled_source_is_fatal_when_startup_grace_ends() {
    let sources = [
        Progress::MainEncoder,
        Progress::SubEncoder,
        Progress::Network,
        Progress::Rtsp,
        Progress::Mqtt,
        Progress::Hardware,
    ];
    for stalled in sources {
        let rig = Rig::new();
        let run = rig.start(Options::default());
        until("services", || rig.saw("rtsp_started"));
        while rig.clock.secs() < 20 {
            rig.clock.tick();
        }
        assert!(!run.is_finished(), "{stalled:?}: healthy so far");
        match stalled {
            Progress::MainEncoder => rig.control().muted.store(1, Release),
            Progress::SubEncoder => rig.control().muted.store(2, Release),
            Progress::Network => rig.pauses.net.store(true, Release),
            Progress::Rtsp => rig.pauses.rtsp.store(true, Release),
            Progress::Mqtt => rig.pauses.mqtt.store(true, Release),
            Progress::Hardware => rig.pauses.hw.store(true, Release),
        }
        let feeds_at_stall = rig.feeds.load(Acquire);
        let at = rig.drive_until_done(&run, 200);
        assert!(run.is_finished(), "{stalled:?}: never detected");
        // Before grace ends the camera may still be starting, so feeding continues until 90 s.
        assert!((90..=95).contains(&at), "{stalled:?} detected at {at}s");
        assert!(rig.feeds.load(Acquire) > feeds_at_stall);
        let last = rig.feeds.load(Acquire);
        let outcome = finish(run);
        assert!(outcome.result.is_err(), "{stalled:?}");
        assert_eq!(
            rig.feeds.load(Acquire),
            last,
            "{stalled:?}: fed after failing"
        );
        assert!(!rig.saw("DISARM"));
        assert!(rig.saw("mqtt_exited") && rig.saw("common_uninit"));
    }
}

#[test]
fn sustained_health_qualifies_and_a_later_stall_revokes_it() {
    let rig = Rig::new();
    let video = rig.video();
    let run = rig.start(Options::default());
    until("services", || rig.saw("rtsp_started"));
    let mut observer = HealthObserver::new("boot1-a1", std::process::id());
    let mut qualified_at = None;
    while rig.clock.secs() < 200 {
        rig.step(&video);
        let json = rig.dir.health();
        if observer.observe(json.as_deref(), rig.clock.now()) && qualified_at.is_none() {
            qualified_at = Some(rig.clock.secs());
        }
        assert!(
            !run.is_finished(),
            "healthy run stopped at {}s",
            rig.clock.secs()
        );
    }
    let at = qualified_at.expect("never qualified");
    assert!((180..=190).contains(&at), "qualified at {at}s");
    rig.pauses.net.store(true, Release);
    let mut revoked = false;
    while !run.is_finished() && rig.clock.secs() < 400 {
        rig.clock.tick();
        revoked |= !observer.observe(rig.dir.health().as_deref(), rig.clock.now());
    }
    assert!(revoked, "a stalled source must revoke qualification");
    let outcome = finish(run);
    assert!(outcome.result.is_err());
    assert!(!observer.observe(rig.dir.health().as_deref(), rig.clock.now()));
    assert!(rig.dir.health().is_none());
}

#[test]
fn a_stuck_hdal_poll_withholds_feeding_and_the_watchdog_is_left_to_reset() {
    let rig = Rig::new();
    let lock = rig.dir.0.join("runtime.lock");
    let flag = rig.dir.0.join("wdt_flag");
    let device = rig.dir.0.join("unused-device");
    let acquire = || cam::runtime::s00::S00Handoff::acquire(&lock, &flag, &device);
    let mut wiring = rig.wiring(rig.clock.clone(), Options::default());
    wiring.handoff.lock = Some(acquire().unwrap());
    let run = std::thread::spawn(move || run(wiring));
    until("services", || rig.saw("rtsp_started"));
    while rig.clock.secs() < 30 {
        rig.clock.tick();
    }
    rig.control().block_recv.store(true, Release);
    for _ in 0..80 {
        rig.clock.tick();
    }
    assert!(
        !run.is_finished(),
        "the owner is stuck inside the HDAL poll"
    );
    let frozen = rig.feeds.load(Acquire);
    for _ in 0..30 {
        rig.clock.tick();
    }
    assert_eq!(
        rig.feeds.load(Acquire),
        frozen,
        "no feed while the media poll is stuck"
    );
    assert!(
        !rig.saw("common_uninit"),
        "graph stays alive under a stuck poll"
    );
    assert!(!rig.saw("DISARM"));
    assert!(
        acquire().is_err(),
        "lock must survive feeder failure and stuck HDAL"
    );
    assert!(
        cam::ownership::shared_diagnostic(&lock).is_err(),
        "diagnostics cannot revive feeding under a stuck graph"
    );
    rig.control().block_recv.store(false, Release);
    let outcome = finish(run);
    assert!(outcome.result.is_err());
    assert!(
        acquire().is_err(),
        "outcome retains ownership until terminal exit"
    );
    drop(outcome);
    assert!(acquire().is_ok());
}

#[test]
fn a_join_that_does_not_finish_keeps_hdal_alive_and_feeding_cancelled() {
    let rig = Rig::new();
    rig.hold_join.store(true, Release);
    let run = rig.start(Options::default());
    until("services", || rig.saw("rtsp_started"));
    while rig.clock.secs() < 10 {
        rig.clock.tick();
    }
    rig.stop.store(true, Release);
    until("mqtt worker stopping", || {
        rig.control().muted.load(Acquire) == 0 && rig.saw("rtsp_exited")
    });
    let feeds_when_stopping = rig.feeds.load(Acquire);
    for _ in 0..70 {
        rig.clock.tick();
    }
    assert!(!run.is_finished(), "join must not be abandoned");
    assert!(!rig.saw("audio_cap_stop") && !rig.saw("common_uninit"));
    assert!(
        rig.feeds.load(Acquire) <= feeds_when_stopping + 1,
        "feeding must stop once stop is requested"
    );
    rig.hold_join.store(false, Release);
    rig.drive_until_done(&run, 1000);
    let outcome = finish(run);
    assert!(outcome.result.is_ok());
    assert!(rig.at("mqtt_exited").unwrap() < rig.at("common_uninit").unwrap());
}

#[test]
fn a_blocked_hardware_worker_that_never_homes_ages_out_after_grace() {
    let rig = Rig::new();
    rig.pauses.hw.store(true, Release); // initialization/homing never completes
    let run = rig.start(Options::default());
    until("services", || rig.saw("rtsp_started"));
    let at = rig.drive_until_done(&run, 200);
    assert!((90..=95).contains(&at), "detected at {at}s");
    assert!(finish(run).result.is_err());
}

#[test]
fn feeder_failure_after_start_is_propagated_even_though_media_keeps_running() {
    let rig = Rig::new();
    let run = rig.start(Options {
        fail_keepalive_at: Some(3),
        ..Options::default()
    });
    until("services", || rig.saw("rtsp_started"));
    let at = rig.drive_until_done(&run, 100);
    assert!(at <= 40, "feeder failure noticed at {at}s");
    let outcome = finish(run);
    assert!(err(&outcome).contains("watchdog"), "{}", err(&outcome));
    assert!(rig.saw("common_uninit"), "media torn down after the join");
    assert!(!rig.saw("DISARM"));
}

#[test]
fn a_stop_signal_cancels_feeding_even_while_startup_waits() {
    let rig = Rig::new();
    rig.stop.store(true, Release);
    let outcome = finish(rig.start(Options::default()));
    assert!(outcome.result.is_err());
    assert!(!rig.saw("common_init"));
    assert!(!rig.saw("DISARM"));
}

#[test]
fn feeder_unwind_retains_the_real_runtime_lock_until_the_outcome_is_dropped() {
    use cam::runtime::s00::S00Handoff;
    let rig = Rig::new();
    let lock = rig.dir.0.join("runtime.lock");
    let flag = rig.dir.0.join("wdt_flag");
    let device = rig.dir.0.join("unused-device");
    let mut wiring = rig.wiring(Monotonic::default(), Options::default());
    wiring.handoff.lock = Some(S00Handoff::acquire(&lock, &flag, &device).unwrap());
    wiring.handoff.panic_open = true;
    let outcome = run(wiring);
    assert!(err(&outcome).contains("panicked"));
    assert!(!rig.saw("common_init"));
    assert!(
        S00Handoff::acquire(&lock, &flag, &device).is_err(),
        "feeder unwind released ownership before terminal exit"
    );
    drop(outcome);
    assert!(S00Handoff::acquire(&lock, &flag, &device).is_ok());
}

#[test]
fn cancellation_while_opening_watchdog_does_not_arm_or_feed_it() {
    let rig = Rig::new();
    let mut wiring = rig.wiring(Monotonic::default(), Options::default());
    wiring.handoff.stop_on_open = Some(rig.stop);
    let outcome = run(wiring);
    assert!(outcome.result.is_err());
    assert!(!rig.saw("common_init"));
    assert!(!rig.saw("wdt_arm"), "armed after cancellation during open");
    assert_eq!(rig.feeds.load(Acquire), 0);
    assert!(outcome.handoff.is_some());
}

/// The feeder has published time zero and stalls; the startup owner resumes four seconds later.
#[derive(Clone)]
struct StaleReadinessClock {
    stop: &'static AtomicBool,
}
impl Clock for StaleReadinessClock {
    fn now(&self) -> Duration {
        if std::thread::current().name() == Some("watchdog") {
            Duration::ZERO
        } else {
            Duration::from_secs(4)
        }
    }
    fn wait(&mut self, _: Duration) {
        while !self.stop.load(Acquire) {
            nap();
        }
    }
}

#[test]
fn stale_first_publication_cannot_authorize_hardware_startup() {
    let rig = Rig::new();
    let wiring = rig.wiring(StaleReadinessClock { stop: rig.stop }, Options::default());
    let runner = std::thread::spawn(move || run(wiring));
    until("startup decision", || {
        runner.is_finished() || rig.saw("common_init")
    });
    rig.stop.store(true, Release);
    let outcome = finish(runner);
    assert!(
        !rig.saw("common_init"),
        "stale queued readiness started hardware"
    );
    assert!(!rig.saw("network_started"));
    assert!(outcome.result.is_err());
    assert!(outcome.handoff.is_some());
}

#[derive(Clone)]
struct FailedReadinessClock {
    stop: &'static AtomicBool,
    record: PathBuf,
    panic: bool,
}
impl Clock for FailedReadinessClock {
    fn now(&self) -> Duration {
        if std::thread::current().name() != Some("watchdog") {
            // Delay the startup observer until the feeder has invalidated its queued record.
            until("feeder invalidation", || !self.record.exists());
        }
        Duration::ZERO
    }
    fn wait(&mut self, _: Duration) {
        assert!(!self.panic, "feeder died after publishing readiness");
        self.stop.store(true, Release);
    }
}

#[test]
fn queued_readiness_is_revoked_by_feeder_death_or_signal() {
    for panic in [false, true] {
        let rig = Rig::new();
        let wiring = rig.wiring(
            FailedReadinessClock {
                stop: rig.stop,
                record: rig.dir.0.join("cam-health-boot1-a1.json"),
                panic,
            },
            Options::default(),
        );
        let outcome = run(wiring);
        assert!(outcome.result.is_err());
        assert!(!rig.saw("common_init"));
        assert!(!rig.saw("network_started"));
        assert!(outcome.handoff.is_some());
    }
}

#[test]
fn cancellation_during_timeout_ioctl_prevents_the_initial_keepalive() {
    let rig = Rig::new();
    let mut wiring = rig.wiring(Monotonic::default(), Options::default());
    wiring.handoff.dev.as_mut().unwrap().stop_on_arm = Some(rig.stop);
    let outcome = run(wiring);
    assert!(outcome.result.is_err());
    assert_eq!(
        rig.feeds.load(Acquire),
        0,
        "fed after cancellation during SETTIMEOUT"
    );
    assert!(!rig.saw("common_init"));
    assert!(!rig.saw("DISARM"));
}

#[test]
fn a_keepalive_error_is_not_hidden_by_a_concurrent_orderly_stop() {
    let rig = Rig::new();
    let mut wiring = rig.wiring(
        rig.clock.clone(),
        Options {
            fail_keepalive_at: Some(2),
            ..Options::default()
        },
    );
    wiring.handoff.dev.as_mut().unwrap().stop_on_feed_error = Some(rig.stop);
    let runner = std::thread::spawn(move || run(wiring));
    until("services", || rig.saw("rtsp_started"));
    rig.drive_until_done(&runner, 30);
    let outcome = finish(runner);
    assert!(err(&outcome).contains("keepalive"));
}

#[test]
fn first_publication_failure_starts_no_hardware_or_services() {
    let rig = Rig::new();
    let mut wiring = rig.wiring(Monotonic::default(), Options::default());
    let record = rig.dir.0.join("cam-health-boot1-a1.json");
    wiring.handoff.on_open = Some(Box::new(move || {
        // Directory collision after the writable check makes atomic publication fail.
        std::fs::create_dir(record).unwrap();
    }));
    let outcome = run(wiring);
    assert!(outcome.result.is_err());
    assert!(!rig.saw("common_init") && !rig.saw("network_started"));
    assert!(outcome.handoff.is_some());
    assert_eq!(rig.feeds.load(Acquire), 1);
}

#[test]
fn late_watchdog_open_is_rejected_without_arming() {
    let rig = Rig::new();
    let mut wiring = rig.wiring(rig.clock.clone(), Options::default());
    let time = Arc::clone(&rig.clock.ms);
    wiring.handoff.on_open = Some(Box::new(move || {
        time.store(10_000, Release);
    }));
    let outcome = run(wiring);
    assert!(err(&outcome).contains("deadline"));
    assert!(!rig.saw("wdt_arm") && !rig.saw("common_init"));
    assert!(outcome.handoff.is_some());
}

#[test]
fn stop_during_busy_acquisition_joins_without_starting_hardware() {
    let rig = Rig::new();
    let runner = rig.start(Options {
        open_error: Some(libc::EBUSY),
        ..Options::default()
    });
    until("acquisition retry", || rig.clock.waits.load(Acquire) > 0);
    rig.stop.store(true, Release);
    rig.clock.closed.store(true, Release);
    let outcome = finish(runner);
    assert!(outcome.result.is_err());
    assert!(outcome.handoff.is_some());
    assert!(!rig.saw("common_init") && !rig.saw("network_started"));
    assert_eq!(rig.feeds.load(Acquire), 0);
}

#[test]
fn real_network_failure_during_graph_opening_cancels_further_startup() {
    let rig = Rig::new();
    *rig.control.hold_step.lock().unwrap() = Some(Step::CommonInit);
    // The real network service cannot acquire its Host boundary. Hold the first HDAL call
    // until that error has happened, reproducing failure during startup rather than polling.
    let slot = Arc::new(Mutex::new(None::<cam::net::linux::LinuxHost>));
    let blocked = slot.lock().unwrap();
    let service = cam::runtime::production::net_service(
        Arc::clone(&slot),
        cam::net::Timing::default(),
        Duration::ZERO,
    );
    let wiring = rig.wiring(
        Monotonic::default(),
        Options {
            network: Some(service),
            ..Options::default()
        },
    );
    let runner = std::thread::spawn(move || run(wiring));
    until("graph opening", || rig.saw("common_init"));
    drop(blocked);
    // Invalidating health proves the worker's lease has ended; the feeder independently stops.
    until("worker failure invalidates health", || {
        rig.dir.health().is_none()
    });
    *rig.control.hold_step.lock().unwrap() = None;
    let outcome = finish(runner);
    assert!(err(&outcome).contains("network host unavailable"));
    assert!(
        !rig.saw("common_mem_init"),
        "graph kept building after required worker death"
    );
    assert!(!rig.saw("mqtt_started") && !rig.saw("rtsp_started"));
    assert!(rig.saw("common_uninit"));
    assert!(outcome.handoff.is_some());
}

type NetworkSlot = Arc<Mutex<Option<cam::net::linux::LinuxHost>>>;

/// Real start() service assembly and LinuxHost, with only the watchdog and hardware replaced.
fn real_services(
    rig: &Rig,
    opener: cam::runtime::production::HardwareOpener,
) -> (
    Wiring<Hand, Fake, ManualClock>,
    NetworkSlot,
    std::net::TcpListener,
) {
    use std::os::unix::fs::PermissionsExt;
    let root = &rig.dir.0;
    let write = |name: &str, bytes: &str| {
        let path = root.join(name.trim_start_matches('/'));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        if name.starts_with("mnt/para/") {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        path
    };
    for artifact in cam::preflight::ARTIFACTS {
        let path = write(artifact, "fixture");
        if artifact.starts_with("/sbin/") {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    std::fs::remove_file(root.join("dev/mtd7ro")).unwrap();
    std::os::unix::fs::symlink("/dev/null", root.join("dev/mtd7ro")).unwrap();
    write(
        "mnt/para/cam-wifi.conf",
        "network={\nssid=\"fixture-ap\"\npsk=\"fixture-password\"\n}\n",
    );
    write("mnt/para/cam-rtsp.auth", "fixture:password\n");
    // A reserved local socket never responds, exercising real MQTT connection timeouts.
    let broker = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    write(
        "mnt/para/cam.toml",
        &format!(
            "[mqtt]\nhost='127.0.0.1'\nport={}\n",
            broker.local_addr().unwrap().port()
        ),
    );
    write("sys/class/gpio/gpio133/direction", "out");
    write("sys/class/gpio/gpio133/value", "1");
    write("sys/class/net/wlan0/flags", "0x1");
    write("sys/class/net/wlan0/carrier", "1");
    write("proc/modules", "ntkimethmac 0 0\n");
    std::fs::create_dir_all(root.join("mnt/tmp")).unwrap();
    for name in ["insmod", "rmmod", "ifconfig", "modprobe"] {
        let path = write(&format!("sbin/{name}"), "#!/bin/sh\nexit 0\n");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    for name in ["wpa_supplicant", "udhcpc"] {
        let path = write(
            &format!("sbin/{name}"),
            &format!(
                "#!/bin/sh\necho $$ > '{}/{name}.pid'\nexec sleep 60\n",
                root.display()
            ),
        );
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let fake = rig.wiring(rig.clock.clone(), Options::default()).hdal;
    let started = cam::runtime::production::start(
        &cam::runtime::production::Paths::rooted(root, 0),
        "boot1-a1",
        rig.stop,
        fake,
        Monotonic::default(),
        opener,
    )
    .unwrap();
    let mut handoff = rig.wiring(rig.clock.clone(), Options::default()).handoff;
    handoff.lock = Some(started.wiring.handoff);
    (
        Wiring {
            handoff,
            clock: rig.clock.clone(),
            health_dir: rig.dir.0.clone(),
            attempt: started.wiring.attempt,
            pid: started.wiring.pid,
            stop: started.wiring.stop,
            hdal: started.wiring.hdal,
            media: started.wiring.media,
            hubs: started.wiring.hubs,
            audio: started.wiring.audio,
            talk: started.wiring.talk,
            pipeline: started.wiring.pipeline,
            media_tuning: started.wiring.media_tuning,
            counters: started.wiring.counters,
            services: started.wiring.services,
        },
        started.network,
        broker,
    )
}

struct HardwareFault {
    trace: Trace,
    hold: Flag,
    panic_tick: bool,
}
impl cam::mqtt::Hardware for HardwareFault {
    fn apply(&mut self, _: &cam::mqtt::Command) -> Result<(), String> {
        Ok(())
    }
    fn tick(&mut self, _: Duration) -> Result<Option<Duration>, String> {
        self.trace.lock().unwrap().push("hardware_tick");
        while self.hold.load(Acquire) {
            nap();
        }
        assert!(!self.panic_tick, "nested ISP fault");
        Ok(None)
    }
    fn states(&mut self) -> Result<std::collections::BTreeMap<String, String>, String> {
        Ok(Default::default())
    }
}
impl Drop for HardwareFault {
    fn drop(&mut self) {
        self.trace.lock().unwrap().push("hardware_dropped");
    }
}

#[test]
fn real_start_run_joins_nested_mqtt_failure_before_teardown_and_retains_network() {
    let rig = Rig::new();
    let trace = Arc::clone(&rig.trace);
    let (wiring, network, _broker) = real_services(
        &rig,
        Box::new(move |_| {
            Ok(Box::new(HardwareFault {
                trace,
                hold: flag(false),
                panic_tick: true,
            }))
        }),
    );
    let runner = std::thread::spawn(move || run(wiring));
    until("nested hardware tick", || rig.saw("hardware_tick"));
    rig.drive_until_done(&runner, 20);
    rig.clock.closed.store(true, Release);
    let outcome = finish(runner);
    assert!(
        err(&outcome).contains("hardware worker panicked"),
        "{}",
        err(&outcome)
    );
    assert!(rig.at("hardware_dropped").unwrap() < rig.at("audio_cap_stop").unwrap());
    assert!(rig.at("hardware_dropped").unwrap() < rig.at("common_uninit").unwrap());
    assert!(
        network.lock().unwrap().is_some(),
        "real network host lost on failure"
    );
    let mut host = network.lock().unwrap().take().unwrap();
    use cam::net::Host;
    assert!(host.alive(cam::net::Proc::Wpa).unwrap());
    assert!(host.alive(cam::net::Proc::DhcpWlan).unwrap());
    host.shutdown(); // Only the isolated fixture is cleaned up; production uses terminal exit.
    assert!(outcome.handoff.is_some());
}

#[test]
fn real_hardware_open_failure_or_unwind_is_owned_during_startup() {
    for panic in [false, true] {
        let rig = Rig::new();
        let trace = Arc::clone(&rig.trace);
        let (wiring, network, _broker) = real_services(
            &rig,
            Box::new(move |_| {
                trace.lock().unwrap().push("hardware_open");
                assert!(!panic, "open unwound");
                Err("hardware open failed".into())
            }),
        );
        let runner = std::thread::spawn(move || run(wiring));
        until("hardware open", || rig.saw("hardware_open"));
        rig.drive_until_done(&runner, 20);
        rig.clock.closed.store(true, Release);
        let outcome = finish(runner);
        assert!(err(&outcome).contains(if panic {
            "mqtt panicked"
        } else {
            "hardware open failed"
        }));
        assert!(rig.saw("common_uninit"));
        assert!(!rig.saw("hardware_tick"));
        assert!(network.lock().unwrap().is_some());
    }
}

#[test]
fn real_nested_hardware_join_retains_graph_network_and_runtime_lock() {
    let rig = Rig::new();
    rig.hold_join.store(true, Release);
    let (trace, hold) = (Arc::clone(&rig.trace), Arc::clone(&rig.hold_join));
    let (wiring, network, _broker) = real_services(
        &rig,
        Box::new(move |_| {
            Ok(Box::new(HardwareFault {
                trace,
                hold,
                panic_tick: false,
            }))
        }),
    );
    let runner = std::thread::spawn(move || run(wiring));
    until("nested hardware call", || rig.saw("hardware_tick"));
    rig.stop.store(true, Release);
    until("network returned", || network.lock().unwrap().is_some());
    rig.clock.tick();
    let feeds = rig.feeds.load(Acquire);
    let lock = rig.dir.0.join("mnt/tmp/cam-runtime.lock");
    let flag = rig.dir.0.join("unused-flag");
    let dev = rig.dir.0.join("unused-device");
    let acquire = || cam::runtime::s00::S00Handoff::acquire(&lock, &flag, &dev);
    for _ in 0..3 {
        rig.clock.tick();
    }
    assert!(!runner.is_finished());
    assert!(!rig.saw("audio_cap_stop") && !rig.saw("common_uninit"));
    assert_eq!(rig.feeds.load(Acquire), feeds);
    assert!(acquire().is_err());
    assert!(cam::ownership::shared_diagnostic(&lock).is_err());
    rig.hold_join.store(false, Release);
    rig.clock.closed.store(true, Release);
    let outcome = finish(runner);
    assert!(outcome.result.is_ok(), "{:?}", outcome.result);
    assert!(rig.at("hardware_dropped").unwrap() < rig.at("common_uninit").unwrap());
    assert!(acquire().is_err());
    assert!(cam::ownership::shared_diagnostic(&lock).is_err());
    drop(outcome);
    assert!(acquire().is_ok());
}

#[test]
fn production_network_exit_fixture() {
    let Some(root) = std::env::var_os("CAM_TEST_PRODUCTION_EXIT_ROOT") else {
        return;
    };
    let mut rig = Rig::new();
    rig.dir = TempDir(root.into());
    *rig.control.hold_step.lock().unwrap() = Some(Step::CommonInit);
    let (mut wiring, network, _broker) =
        real_services(&rig, Box::new(|_| Err("not reached".into())));
    let mut hw = Fake::failing(Step::CommonMemInit, -7);
    hw.trace = Some(Arc::clone(&rig.trace));
    hw.control = Some(Arc::clone(&rig.control));
    wiring.hdal = hw;
    let runner = std::thread::spawn(move || run(wiring));
    until("owned network children", || {
        ["wpa_supplicant.pid", "udhcpc.pid"]
            .into_iter()
            .all(|name| {
                std::fs::read_to_string(rig.dir.0.join(name))
                    .is_ok_and(|s| s.trim().parse::<u32>().is_ok())
            })
    });
    *rig.control.hold_step.lock().unwrap() = None;
    rig.clock.closed.store(true, Release);
    let outcome = finish(runner);
    assert!(err(&outcome).contains("media"));
    assert!(rig.saw("common_uninit"));
    assert!(outcome.handoff.is_some());
    let host = network.lock().unwrap().take().unwrap();
    host.exit_preserving_network(23);
}

fn process_running(pid: &str) -> bool {
    std::process::Command::new("/bin/kill")
        .args(["-0", pid])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success()
}

/// Own even the orphaned fixture children if an assertion fails in the parent test.
struct NetworkChildren(PathBuf);
impl Drop for NetworkChildren {
    fn drop(&mut self) {
        for name in ["wpa_supplicant.pid", "udhcpc.pid"] {
            if let Ok(pid) = std::fs::read_to_string(self.0.join(name))
                && pid.trim().parse::<u32>().is_ok()
            {
                let _ = std::process::Command::new("/bin/kill")
                    .args(["-KILL", pid.trim()])
                    .stderr(std::process::Stdio::null())
                    .status();
                until("fixture child reaped", || !process_running(pid.trim()));
            }
        }
    }
}

#[test]
fn real_production_media_failure_preserves_network_children_through_terminal_exit() {
    let root = TempDir::new();
    let cleanup = NetworkChildren(root.0.clone());
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "production_network_exit_fixture", "--nocapture"])
        .env("CAM_TEST_PRODUCTION_EXIT_ROOT", &root.0)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("terminal production fixture hung");
        }
        nap();
    };
    assert_eq!(status.code(), Some(23));
    for name in ["wpa_supplicant.pid", "udhcpc.pid"] {
        let pid = std::fs::read_to_string(root.0.join(name)).unwrap();
        assert!(
            process_running(pid.trim()),
            "{name} died with production owner"
        );
    }
    drop(cleanup);
}

#[test]
fn real_network_unwind_returns_live_children_while_media_poll_is_held() {
    let rig = Rig::new();
    rig.control.block_recv.store(true, Release);
    let trace = Arc::clone(&rig.trace);
    let (wiring, network, _broker) = real_services(
        &rig,
        Box::new(move |_| {
            Ok(Box::new(HardwareFault {
                trace,
                hold: flag(false),
                panic_tick: false,
            }))
        }),
    );
    let runner = std::thread::spawn(move || run(wiring));
    until("network children", || {
        ["wpa_supplicant.pid", "udhcpc.pid"]
            .into_iter()
            .all(|name| {
                std::fs::read_to_string(rig.dir.0.join(name))
                    .is_ok_and(|s| s.trim().parse::<u32>().is_ok())
            })
    });
    until("held media poll", || rig.saw("videoenc_recv"));
    rig.clock.panic_network.store(true, Release);
    until("network returned after unwind", || {
        network.lock().unwrap().is_some()
    });
    rig.clock.tick();
    assert!(!rig.saw("common_uninit"));
    {
        use cam::net::Host;
        let mut slot = network.lock().unwrap();
        let host = slot.as_mut().unwrap();
        assert!(host.alive(cam::net::Proc::Wpa).unwrap());
        assert!(host.alive(cam::net::Proc::DhcpWlan).unwrap());
    }
    rig.control.block_recv.store(false, Release);
    rig.clock.closed.store(true, Release);
    let outcome = finish(runner);
    assert!(err(&outcome).contains("network panicked"));
    assert!(rig.saw("common_uninit"));
    assert!(outcome.handoff.is_some());
}

#[test]
fn cancelled_real_mqtt_service_does_not_open_hardware_when_first_scheduled() {
    for signal in [true, false] {
        let rig = Rig::new();
        let opened = flag(false);
        let observed = Arc::clone(&opened);
        let (wiring, _network, _broker) = real_services(
            &rig,
            Box::new(move |_| {
                observed.store(true, Release);
                Err("must not open after shutdown".into())
            }),
        );
        let health = cam::runtime::LifecycleHealth::new();
        if signal {
            rig.stop.store(true, Release);
        } else {
            health.stop();
        }
        let beats = cam::runtime::Beats::new(health, rig.clock.clone());
        (wiring.services.mqtt)(beats, rig.stop).unwrap();
        assert!(
            !opened.load(Acquire),
            "a delayed MQTT thread opened hardware after stop"
        );
    }
}

#[test]
fn cancellation_during_hardware_open_does_not_start_nested_mqtt_workers() {
    let rig = Rig::new();
    let (stop, trace) = (rig.stop, Arc::clone(&rig.trace));
    let (wiring, _network, _broker) = real_services(
        &rig,
        Box::new(move |_| {
            stop.store(true, Release);
            Ok(Box::new(HardwareFault {
                trace,
                hold: flag(false),
                panic_tick: false,
            }))
        }),
    );
    let health = cam::runtime::LifecycleHealth::new();
    let beats = cam::runtime::Beats::new(health.clone(), rig.clock.clone());
    (wiring.services.mqtt)(beats, rig.stop).unwrap();
    assert_eq!(health.last_beat(Progress::Hardware), None);
    assert!(!rig.saw("hardware_tick"));
    assert!(rig.saw("hardware_dropped"));
}
