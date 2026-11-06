//! The production `cam serve --attempt TOKEN` lifecycle owner.
//!
//! One owner for the watchdog feeder, network, HDAL media, RTSP and MQTT (with its nested hardware
//! worker). Every OS-facing piece is injected, so host tests drive this exact orchestration:
//!
//! ```text
//! HealthFile::new -> feeder thread (S00 handoff, arm, publish) -> wait first Some status
//!   -> network worker -> stream_with_audio { on_ready: mqtt + rtsp workers,
//!      keep_going: worker/feeder liveness, on_stopping: LifecycleHealth::stop THEN join workers }
//!   -> LifecycleHealth::stop -> join feeder -> Outcome
//! ```
//!
//! Nothing here disarms the watchdog: every return, success included, leaves the feeder stopped, so
//! the armed hardware timer resets the boot. The caller keeps `Outcome::handoff` (the runtime lock)
//! until the process exits and then calls `LinuxHost::exit_preserving_network`.

use std::cell::RefCell;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use hdal::{AudioCap, AudioOut, Backend, enc::EncBackend};

use crate::exclusion;
use crate::mqtt::{Hardware, PipelineHealth};
use crate::net::linux::LinuxHost;
use crate::net::{Host, Supervisor, Timing};
use crate::ownership::RUNTIME_LOCK;
use crate::pipeline::{self, AudioReport, PipelineConfig};
use crate::preflight;
use crate::rtsp::server as rtsp_server;
use crate::rtsp::{audio::AudioHub, hub::VideoHubs, talk::Talkback};
use crate::runtime::s00::S00Handoff;
use crate::runtime::workers::Workers;
use crate::runtime::{
    Beats, Clock, Handoff, HealthFile, HealthStatus, LifecycleHealth, MAX_POLL_GAP, Monotonic,
    Progress, WorkerLease, supervise, valid_attempt,
};

/// Network supervisor cadence. The worst tick (seven sequential 2 s helpers plus cleanup) stays
/// inside the 30 s `Network` budget; this pause adds to it.
pub const NET_TICK: Duration = Duration::from_secs(2);
/// Network events printed per boot; later ones are counted in one final line.
pub const NET_LOG_LINES: u32 = 64;
const FEEDER_EXITED: &str = "watchdog feeder exited";
const FIRST_STATUS_POLL: Duration = Duration::from_millis(20);

/// A required worker body. It runs on its own thread with a `WorkerLease` held around it.
pub type Service = Box<dyn FnOnce(Beats, &'static AtomicBool) -> io::Result<()> + Send>;

pub struct Services {
    pub network: Service,
    pub rtsp: Service,
    pub mqtt: Service,
}

pub struct Wiring<H, B, C> {
    /// Already holds the exclusive runtime lock; `supervise` requests the S00 release and arms.
    pub handoff: H,
    /// Created at the start of the attempt, before the handoff; shared by every producer.
    pub clock: C,
    pub health_dir: PathBuf,
    pub attempt: String,
    pub pid: u32,
    pub stop: &'static AtomicBool,
    pub hdal: B,
    pub media: PipelineConfig,
    pub hubs: VideoHubs,
    pub audio: Arc<AudioHub>,
    pub talk: Arc<Talkback>,
    pub pipeline: Arc<PipelineHealth>,
    /// Persisted startup values and live requests from the settings owner.
    pub media_tuning: Arc<crate::media_tuning::Handoff>,
    /// Delivered frames/bytes per stream, read by the telemetry thread.
    pub counters: Arc<crate::telemetry::StreamCounters>,
    pub services: Services,
}

pub struct Outcome<H> {
    pub result: io::Result<AudioReport>,
    /// Keep alive until process exit: dropping it releases the runtime lock.
    pub handoff: Option<H>,
}

fn nap(duration: Duration, stop: &AtomicBool) {
    let end = std::time::Instant::now() + duration;
    while !stop.load(Ordering::Relaxed) {
        let left = end.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return;
        }
        std::thread::sleep(left.min(Duration::from_millis(100)));
    }
}

/// The network worker: one supervisor tick, then a `Network` beat, then a pause. Recoverable Wi-Fi/DHCP
/// faults are events inside a completed tick, so retries keep the heartbeat; a tick that blocks does not.
/// The host goes back into `slot` on stop or unwind so the process can leave it running (it never
/// drops it: `LinuxHost::drop` would kill the supplicant and DHCP clients).
pub fn net_service<N: Host + Send + 'static>(
    slot: Arc<Mutex<Option<N>>>,
    timing: Timing,
    tick: Duration,
) -> Service {
    Box::new(move |beats, stop| {
        let host = slot
            .lock()
            .ok()
            .and_then(|mut slot| slot.take())
            .ok_or_else(|| io::Error::other("network host unavailable"))?;
        let mut owner = NetworkReturn {
            host: Some(host),
            slot: &slot,
        };
        let host = owner
            .host
            .as_mut()
            .ok_or_else(|| io::Error::other("network host unavailable"))?;
        let mut supervisor = Supervisor::new(timing);
        let mut budget = NET_LOG_LINES;
        while !stop.load(Ordering::Relaxed) {
            let events = supervisor.tick(host, beats.now());
            beats.beat(Progress::Network);
            for event in events {
                match budget {
                    0 => {}
                    1 => println!("net: further events suppressed"),
                    _ => println!("net: {:>6.1}s {event:?}", beats.now().as_secs_f32()),
                }
                budget = budget.saturating_sub(1);
            }
            nap(tick, stop);
        }
        Ok(())
    })
}

/// Return ownership even when a worker unwinds. No mutex is held while supervising the network,
/// and a poisoned parking slot must not turn recovery into destructive LinuxHost cleanup.
struct NetworkReturn<'a, N> {
    host: Option<N>,
    slot: &'a Mutex<Option<N>>,
}

impl<N> Drop for NetworkReturn<'_, N> {
    fn drop(&mut self) {
        *self.slot.lock().unwrap_or_else(|error| error.into_inner()) = self.host.take();
    }
}

/// Opens the lens/PTZ/IR hardware inside the MQTT worker, under its `WorkerLease`, so a failure or panic
/// during homing cancels the startup grace. It receives the settings owner. Returns an error string for the log.
pub type HardwareOpener =
    Box<dyn FnOnce(crate::settings::Owner) -> Result<Box<dyn Hardware>, String> + Send>;

/// Where the production runtime keeps its files. `system()` is the camera; tests root it in a fixture.
#[derive(Clone, Debug)]
pub struct Paths {
    root: PathBuf,
    rtsp_port: u16,
}

impl Paths {
    pub fn system() -> Self {
        Self::rooted(Path::new("/"), 554)
    }

    pub fn rooted(root: &Path, rtsp_port: u16) -> Self {
        Self {
            root: root.to_owned(),
            rtsp_port,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn under(&self, path: &str) -> PathBuf {
        self.root.join(path.trim_start_matches('/'))
    }

    /// Persistent inode for the whole boot; never unlinked.
    pub fn lock(&self) -> PathBuf {
        self.under(RUNTIME_LOCK)
    }

    pub fn flag(&self) -> PathBuf {
        self.under(S00_FLAG)
    }

    pub fn settings(&self) -> PathBuf {
        self.under(crate::settings::PATH)
    }

    pub fn focus_map(&self) -> PathBuf {
        self.under(FOCUS_MAP)
    }

    pub fn presets(&self) -> PathBuf {
        self.under(PRESETS)
    }

    pub fn stock_isp(&self) -> PathBuf {
        self.under(STOCK_ISP)
    }
}

/// The vendor's image settings; read once for first-run defaults (never written).
pub const STOCK_ISP: &str = "/mnt/para/0_isp.cfg";
/// The learned focus map, beside the settings.
pub const FOCUS_MAP: &str = "/mnt/para/cam-focus-map.toml";
/// The preset positions saved over `cam.toml`'s, beside the settings.
pub const PRESETS: &str = "/mnt/para/cam-presets.toml";

pub const S00_FLAG: &str = "/mnt/tmp/wdt_flag";
pub const WATCHDOG_DEVICE: &str = "/dev/watchdog";
pub const HEALTH_DIR: &str = "/mnt/tmp";

#[derive(Debug)]
pub enum StartError {
    Attempt,
    Preflight(Vec<String>),
    Lock(io::Error),
    Conflict(Vec<String>),
    Network(io::Error),
    Bind(io::Error),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Attempt => write!(f, "attempt token must be 1..=64 of [A-Za-z0-9_-]"),
            Self::Preflight(p) => write!(f, "preflight failed: {}", p.join("; ")),
            Self::Lock(e) => write!(
                f,
                "another runtime owns {RUNTIME_LOCK} (or it is unusable): {e}"
            ),
            Self::Conflict(c) => write!(f, "another owner is running: {}", c.join(", ")),
            Self::Network(e) => write!(f, "network preflight: {e}"),
            Self::Bind(e) => write!(f, "rtsp bind: {e}"),
        }
    }
}

impl<B> std::fmt::Debug for Started<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Started").finish_non_exhaustive()
    }
}

pub struct Started<B> {
    pub wiring: Wiring<S00Handoff, B, Monotonic>,
    /// The network host is parked here after the network worker stops; the caller keeps it running
    /// by calling `LinuxHost::exit_preserving_network`, never by dropping it.
    pub network: Arc<Mutex<Option<LinuxHost>>>,
}

/// Everything that must succeed before the watchdog handoff: attempt token, strict preflight, the
/// exclusive runtime lock (kept in the returned wiring), process exclusion, the network credentials
/// preflight and the RTSP bind. Nothing here touches `wdt_flag`, `/dev/watchdog`, the network or HDAL.
pub fn start<B>(
    paths: &Paths,
    attempt: &str,
    stop: &'static AtomicBool,
    hdal: B,
    clock: Monotonic,
    open_hardware: HardwareOpener,
) -> Result<Started<B>, StartError> {
    if !valid_attempt(attempt) {
        return Err(StartError::Attempt);
    }
    let ready = preflight::check(paths.root()).map_err(StartError::Preflight)?;
    let handoff = S00Handoff::acquire(&paths.lock(), &paths.flag(), &paths.under(WATCHDOG_DEVICE))
        .map_err(StartError::Lock)?;
    let found = exclusion::conflicts(&paths.under("/proc"), std::process::id())
        .map_err(|e| StartError::Conflict(vec![format!("cannot scan /proc: {e}")]))?;
    if !found.is_empty() {
        return Err(StartError::Conflict(found));
    }
    let host =
        LinuxHost::production_at(paths.root(), &ready.mqtt.camera).map_err(StartError::Network)?;
    let listener = rtsp_server::bind(paths.rtsp_port).map_err(StartError::Bind)?;

    let hubs = VideoHubs::new();
    let audio = AudioHub::new();
    let talk = crate::rtsp::opus::talkback(hdal::audio::RATE);
    let pipeline = Arc::new(PipelineHealth::default());
    let rtsp_config = Arc::new(rtsp_server::Config {
        streams: hubs.streams(),
        creds: Some(ready.rtsp),
        audio: Some(Arc::clone(&audio)),
        talk: Some(Arc::clone(&talk)),
    });
    let network = Arc::new(Mutex::new(Some(host)));
    // Settings never fail startup: an unreadable stock file only loses its first-run defaults.
    let stock_isp = crate::files::read_device_or_file(&paths.stock_isp(), 16 * 1024)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_default();
    let owner = crate::settings::Owner::load(
        Box::new(crate::settings::FileDisk::new(paths.settings())),
        &stock_isp,
        Box::new(crate::settings::FileDisk::new(paths.focus_map())),
        ready.mqtt.presets.clone(),
        Box::new(crate::settings::FileDisk::new(paths.presets())),
    );
    let media_tuning = Arc::clone(&owner.media);
    let counters = Arc::new(crate::telemetry::StreamCounters::default());
    let telemetry = telemetry_task(Arc::clone(&counters), Arc::clone(&media_tuning));
    let mqtt_config = ready.mqtt;
    let mqtt_health = Arc::clone(&pipeline);
    let services = Services {
        network: net_service(Arc::clone(&network), Timing::default(), NET_TICK),
        rtsp: Box::new(move |beats, stop| {
            rtsp_server::serve_with(&listener, rtsp_config, stop, Some(&beats))
        }),
        mqtt: Box::new(move |beats, stop| {
            if stop.load(Ordering::Acquire) || beats.stopped() {
                return Ok(());
            }
            let hardware = open_hardware(owner).map_err(io::Error::other)?;
            if stop.load(Ordering::Acquire) || beats.stopped() {
                return Ok(());
            }
            crate::mqtt::run_full(
                mqtt_config,
                hardware,
                mqtt_health,
                stop,
                Some(&beats),
                Some(telemetry),
            )
        }),
    };
    Ok(Started {
        wiring: Wiring {
            handoff,
            clock,
            health_dir: paths.under(HEALTH_DIR),
            attempt: attempt.to_owned(),
            pid: std::process::id(),
            stop,
            hdal,
            media: PipelineConfig::e1_zoom(),
            hubs,
            audio,
            talk,
            pipeline,
            media_tuning,
            counters,
            services,
        },
        network,
    })
}

/// The telemetry thread's work: system readings plus the counters, with the main encoder's rate as
/// the media thread reports it and the fixed sub rate.
pub fn telemetry_task(
    counters: Arc<crate::telemetry::StreamCounters>,
    media: Arc<crate::media_tuning::Handoff>,
) -> crate::telemetry::Task {
    let sub_fps = PipelineConfig::e1_zoom().streams[1].fps;
    crate::telemetry::Task {
        collector: crate::telemetry::Collector::new(Box::new(crate::telemetry::System), counters),
        expected_fps: Box::new(move || [media.applied().1.map(|m| m.fps), Some(sub_fps)]),
    }
}

type Feeder = JoinHandle<io::Result<()>>;
type HandoffOwner<H> = Arc<Mutex<Option<H>>>;

fn reclaim<H>(owner: HandoffOwner<H>) -> Option<H> {
    owner
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take()
}

/// Mutable orchestration state shared by the media callbacks.
struct Run<'a> {
    stop: &'static AtomicBool,
    health: &'a LifecycleHealth,
    workers: RefCell<Workers<'static>>,
    errors: RefCell<Vec<String>>,
}

impl Run<'_> {
    /// Terminal for this boot: cancel feeding first, then ask everything to stop.
    fn fail(&self, what: impl std::fmt::Display) {
        self.health.stop();
        self.stop.store(true, Ordering::Relaxed);
        self.errors.borrow_mut().push(what.to_string());
    }

    fn spawn(&self, name: &'static str, beats: &Beats, service: Service) -> bool {
        // The lease moves into the worker: a failed thread spawn drops it and cancels feeding.
        let lease: WorkerLease = beats.lease();
        let (beats, stop) = (beats.clone(), self.stop);
        let spawned = self
            .workers
            .borrow_mut()
            .spawn_holding(name, lease, move || service(beats, stop));
        match spawned {
            Ok(()) => true,
            Err(error) => {
                self.fail(format!("{name}: cannot start: {error}"));
                false
            }
        }
    }
}

pub fn run<H, B, C>(wiring: Wiring<H, B, C>) -> Outcome<H>
where
    H: Handoff + Send + 'static,
    B: Backend + EncBackend + AudioCap + AudioOut,
    C: Clock + Clone + Send + Sync + 'static,
{
    let Wiring {
        handoff,
        clock,
        health_dir,
        attempt,
        pid,
        stop,
        mut hdal,
        media,
        hubs,
        audio,
        talk,
        pipeline: pipeline_health,
        media_tuning,
        counters,
        services,
    } = wiring;
    let health = LifecycleHealth::new();
    let beats = Beats::new(health.clone(), clock.clone());
    let mut file = match HealthFile::new(&health_dir, &attempt, pid) {
        Ok(file) => file,
        Err(error) => {
            return Outcome {
                result: Err(error),
                handoff: Some(handoff),
            };
        }
    };

    // The watchdog is not touched until the attempt's record is known to be writable.
    if let Err(error) = file.check_writable() {
        return Outcome {
            result: Err(io::Error::new(
                error.kind(),
                format!("health directory {}: {error}", health_dir.display()),
            )),
            handoff: Some(handoff),
        };
    }

    let (ready_tx, ready) = mpsc::sync_channel::<HealthStatus>(1);
    let feeder_health = health.clone();
    let feeder_clock = clock.clone();
    // The orchestration owner retains the lock even if spawning fails or the feeder unwinds.
    // Only the feeder borrows H while running; reclaim happens after its join.
    let owner = Arc::new(Mutex::new(Some(handoff)));
    let feeder_owner = Arc::clone(&owner);
    let feeder: Feeder =
        match std::thread::Builder::new()
            .name("watchdog".into())
            .spawn(move || {
                let mut owned = feeder_owner
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                let handoff = owned
                    .as_mut()
                    .ok_or_else(|| io::Error::other("handoff unavailable"))?;
                let mut clock = feeder_clock;
                supervise(handoff, &mut clock, feeder_health, stop, |status| {
                    file.publish(status)?;
                    if let Some(status) = status {
                        let _ = ready_tx.try_send(status);
                    }
                    Ok(())
                })
            }) {
            Ok(feeder) => feeder,
            Err(error) => {
                health.stop();
                return Outcome {
                    result: Err(io::Error::other(format!("watchdog: cannot start: {error}"))),
                    handoff: reclaim(owner),
                };
            }
        };

    // A queued publication is only evidence of past readiness. Check its age and the live
    // failure latch after receiving it; feeder death/stop while the owner was descheduled wins.
    let readiness = loop {
        if stop.load(Ordering::Acquire) || health.stopped() || feeder.is_finished() {
            break Err("watchdog stopped before startup");
        }
        match ready.recv_timeout(FIRST_STATUS_POLL) {
            Ok(status) => {
                let fresh = clock
                    .now()
                    .checked_sub(Duration::from_millis(status.elapsed_ms))
                    .is_some_and(|age| age < MAX_POLL_GAP);
                break if fresh
                    && !stop.load(Ordering::Acquire)
                    && !health.stopped()
                    && !feeder.is_finished()
                {
                    Ok(())
                } else {
                    Err("watchdog readiness stale or stopped")
                };
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => break Err("watchdog exited before startup"),
        }
    };
    if let Err(error) = readiness {
        health.stop();
        stop.store(true, Ordering::Release);
        return finish(feeder, owner, vec![error.into()], Err(()));
    }

    let run = Run {
        stop,
        health: &health,
        workers: RefCell::new(Workers::new(stop)),
        errors: RefCell::new(Vec::new()),
    };
    let mut media_result = None;
    // One encoder, on the media thread: every RTSP listener shares its packets through `audio`.
    let mut microphone = crate::rtsp::opus::microphone()
        .map_err(|error| run.fail(error))
        .ok();
    if let Some(microphone) = microphone.as_mut()
        && run.spawn("network", &beats, services.network)
    {
        let Services { rtsp, mqtt, .. } = services;
        let (mut rtsp, mut mqtt) = (Some(rtsp), Some(mqtt));
        let result = pipeline::stream_with_audio(
            &mut hdal,
            &media,
            &mut || {
                if let (Some(mqtt), Some(rtsp)) = (mqtt.take(), rtsp.take())
                    && run.spawn("mqtt", &beats, mqtt)
                {
                    run.spawn("rtsp", &beats, rtsp);
                }
            },
            &mut || {
                // Cancel feeding before any join: a stuck join must end in a watchdog reset.
                health.stop();
                if let Err(error) = run.workers.borrow_mut().shutdown() {
                    run.fail(error);
                }
            },
            &mut || {
                if stop.load(Ordering::Relaxed) {
                    return false;
                }
                if let Err(error) = run.workers.borrow_mut().check() {
                    run.fail(error);
                } else if feeder.is_finished() {
                    run.fail(FEEDER_EXITED);
                } else if health.stopped() {
                    run.fail("required worker stopped");
                }
                !stop.load(Ordering::Relaxed)
            },
            &mut |frame| {
                beats.beat(if frame.stream == 0 {
                    Progress::MainEncoder
                } else {
                    Progress::SubEncoder
                });
                pipeline_health.frame(frame.stream);
                counters.frame(frame.stream, frame.data.len());
                hubs.publish(frame.stream, frame.key, frame.pts_us, frame.data);
            },
            &mut |frame| {
                for (pts_us, packet) in microphone.push(frame.pts_us, &frame.samples) {
                    audio.publish(pts_us, packet);
                }
            },
            &mut || talk.next_pcm(std::time::Instant::now()),
            &mut &*media_tuning,
        );
        media_result = Some(result);
    }
    // Normal and failed exits alike: cancel feeding, then make sure nothing is left running.
    health.stop();
    if let Err(error) = run.workers.borrow_mut().shutdown() {
        run.fail(error);
    }
    let mut errors = run.errors.into_inner();
    let report = match media_result {
        Some(Ok(report)) => Some(report),
        Some(Err(error)) => {
            errors.push(format!("media: {error}"));
            None
        }
        None => None,
    };
    finish(feeder, owner, errors, Ok(report))
}

/// Joins the feeder and builds the outcome. The feeder's own "runtime stopped" error is expected after
/// an orderly stop. Device/publisher/panic errors remain failures even when stop happened concurrently.
fn finish<H>(
    feeder: Feeder,
    owner: HandoffOwner<H>,
    mut errors: Vec<String>,
    media: Result<Option<AudioReport>, ()>,
) -> Outcome<H> {
    let feeder_result = match feeder.join() {
        Ok(result) => result,
        Err(_) => Err(io::Error::other("watchdog feeder panicked")),
    };
    let handoff = reclaim(owner);
    if let Err(error) = feeder_result
        && (!crate::runtime::is_cancelled(&error)
            || media.is_err()
            || errors.iter().any(|e| e == FEEDER_EXITED))
    {
        errors.push(format!("watchdog: {error}"));
    }
    let result = match (errors.is_empty(), media) {
        (true, Ok(Some(report))) => Ok(report),
        (true, _) => Err(io::Error::other("runtime stopped before media started")),
        (false, _) => Err(io::Error::other(errors.join("; "))),
    };
    Outcome { result, handoff }
}
