//! Camera health for Home Assistant's diagnostic sensors: the shared `/proc` readings and change
//! filter (`camera_transport::telemetry`), plus this camera's Wi-Fi signal and the encoders' delivered
//! bitrate, frame rate and shortfall. Sampled on its own thread every [`INTERVAL`]; the media thread
//! only bumps [`StreamCounters`]. A missing reading is unknown, never zero; a value is published only
//! when the [`Reporter`] says so.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use camera_transport::telemetry::{Proc, Reporter, SENSORS, Sampler, Sensor, Source, Thermal};

pub const INTERVAL: Duration = camera_transport::telemetry::SAMPLE_EVERY;

/// The Novatek thermal driver reports whole °C.
pub const THERMAL: Thermal = Thermal::Degrees("/sys/class/thermal/thermal_zone0/temp");

/// The shared catalog sensors this camera publishes, in this order.
pub const IDS: &[&str] = &[
    "cpu_busy",
    "memory_available",
    "daemon_memory",
    "load",
    "uptime",
    "temperature",
    "wifi_signal",
    "main_fps_actual",
    "main_bitrate_actual",
    "main_frames_short",
    "sub_fps_actual",
    "sub_bitrate_actual",
    "sub_frames_short",
];

/// Sensor ids earlier releases published that no longer exist; every MQTT announcement clears their
/// discovery before any config (`transport::Config::retired`).
pub const RETIRED: &[&str] = &[
    "cpu",
    "mem_available",
    "rss",
    "wifi",
    "wifi_rssi",
    "main_dropped",
    "sub_dropped",
];

/// The catalog entries of [`IDS`].
pub fn sensors() -> impl Iterator<Item = &'static Sensor> {
    IDS.iter()
        .filter_map(|id| SENSORS.iter().find(|s| s.id == *id))
}

/// Frames and bytes the encoders delivered, per stream; bumped by the media thread, nothing else.
#[derive(Debug, Default)]
pub struct StreamCounters {
    frames: [AtomicU64; 2],
    bytes: [AtomicU64; 2],
}

impl StreamCounters {
    pub fn frame(&self, stream: usize, bytes: usize) {
        if let (Some(f), Some(b)) = (self.frames.get(stream), self.bytes.get(stream)) {
            f.fetch_add(1, Ordering::Relaxed);
            b.fetch_add(bytes as u64, Ordering::Relaxed);
        }
    }

    fn snapshot(&self) -> [(u64, u64); 2] {
        std::array::from_fn(|i| {
            (
                self.frames[i].load(Ordering::Relaxed),
                self.bytes[i].load(Ordering::Relaxed),
            )
        })
    }
}

/// `signal: -53 dBm` from `iw dev wlan0 link`, the only real dBm on this unit (rtl88x2cu's
/// `/proc/net/wireless` level field is not dBm).
pub fn iw_signal_dbm(link: &str) -> Option<i32> {
    let v = link
        .lines()
        .find_map(|l| l.trim().strip_prefix("signal:"))?
        .trim()
        .strip_suffix("dBm")?
        .trim()
        .parse::<i32>()
        .ok()?;
    (-120..=0).contains(&v).then_some(v)
}

/// Where the Wi-Fi link report comes from; the real one runs `iw`.
pub trait Wifi: Send {
    /// `iw dev wlan0 link` output, bounded in time.
    fn iw_link(&mut self) -> Option<String>;
}

pub struct Iw;

impl Wifi for Iw {
    fn iw_link(&mut self) -> Option<String> {
        let mut child = std::process::Command::new("iw")
            .args(["dev", "wlan0", "link"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()?;
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
            }
        }
        let mut out = String::new();
        use std::io::Read;
        child
            .stdout
            .take()?
            .take(4096)
            .read_to_string(&mut out)
            .ok()?;
        Some(out)
    }
}

/// When, and per-stream `(frames, bytes)`.
type Snapshot = (Duration, [(u64, u64); 2]);

/// One round of readings for every id in [`IDS`]; `None` is unknown.
pub type Readings = Vec<(&'static str, Option<f64>)>;

/// Turns successive samples into readings: system values from the shared sampler, rates from the
/// counters over the window since the previous sample.
pub struct Collector {
    system: Sampler<Box<dyn Source>>,
    wifi: Box<dyn Wifi>,
    counters: std::sync::Arc<StreamCounters>,
    last: Option<Snapshot>,
}

impl Collector {
    pub fn new(
        source: Box<dyn Source>,
        wifi: Box<dyn Wifi>,
        counters: std::sync::Arc<StreamCounters>,
    ) -> Self {
        Self {
            system: Sampler::new(source),
            wifi,
            counters,
            last: None,
        }
    }

    /// This camera's `/proc`, thermal file and `iw`.
    pub fn system(counters: std::sync::Arc<StreamCounters>) -> Self {
        let proc = Proc {
            temperature: Some(THERMAL),
        };
        Self::new(Box::new(proc), Box::new(Iw), counters)
    }

    /// `expected_fps` is what each encoder is configured for; frames short of it over the window
    /// count as short (no HDAL drop counter is established, `docs/settings.md`).
    pub fn sample(&mut self, now: Duration, expected_fps: [Option<u32>; 2]) -> Readings {
        let mut out: Readings = self.system.sample().entries().to_vec();
        out.push((
            "wifi_signal",
            self.wifi
                .iw_link()
                .and_then(|t| iw_signal_dbm(&t))
                .map(f64::from),
        ));
        let counts = self.counters.snapshot();
        let window = self
            .last
            .map(|(then, before)| (now.saturating_sub(then).as_secs_f64(), before))
            .filter(|(secs, _)| *secs > 0.0);
        for (i, name) in ["main", "sub"].into_iter().enumerate() {
            let (fps, kbps, short) = match name {
                "main" => (
                    "main_fps_actual",
                    "main_bitrate_actual",
                    "main_frames_short",
                ),
                _ => ("sub_fps_actual", "sub_bitrate_actual", "sub_frames_short"),
            };
            let rates = window.map(|(secs, before)| {
                let frames = counts[i].0.saturating_sub(before[i].0);
                let bytes = counts[i].1.saturating_sub(before[i].1);
                let short = expected_fps[i].map(|fps| {
                    let expected = (f64::from(fps) * secs).round() as u64;
                    // One frame of slack for the window edges.
                    expected.saturating_sub(frames + 1) as f64
                });
                (
                    frames as f64 / secs,
                    bytes as f64 * 8.0 / 1000.0 / secs,
                    short,
                )
            });
            out.push((fps, rates.map(|r| r.0)));
            out.push((kbps, rates.map(|r| r.1)));
            out.push((short, rates.and_then(|r| r.2)));
        }
        self.last = Some((now, counts));
        out
    }
}

/// What one round asks the MQTT side to do.
#[derive(Debug, Default, PartialEq)]
pub struct Round {
    /// State payloads the change filter lets through.
    pub states: Vec<(&'static str, String)>,
}

/// What the telemetry thread runs: the collector, where it learns each encoder's configured rate, and
/// the change filter in front of the publisher.
pub struct Task {
    pub collector: Collector,
    pub expected_fps: Box<dyn Fn() -> [Option<u32>; 2] + Send>,
    reporter: Reporter,
    connected: bool,
}

impl Task {
    pub fn new(
        collector: Collector,
        expected_fps: Box<dyn Fn() -> [Option<u32>; 2] + Send>,
    ) -> Self {
        let sensors: Vec<Sensor> = sensors().copied().collect();
        Self {
            collector,
            expected_fps,
            reporter: Reporter::new(&sensors),
            connected: false,
        }
    }

    /// Samples at `now`; while the broker is away nothing is offered, and the first round of a
    /// connection starts the filter afresh (a complete replay).
    pub fn round(&mut self, now: Duration, connected: bool) -> Round {
        let fps = (self.expected_fps)();
        let readings = self.collector.sample(now, fps);
        if !connected {
            self.connected = false;
            return Round::default();
        }
        let mut round = Round::default();
        if !self.connected {
            self.connected = true;
            self.reporter.reset();
        }
        round.states = readings
            .into_iter()
            .filter_map(|(id, v)| Some((id, self.reporter.offer(id, v, now)?)))
            .collect();
        round
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camera_transport::telemetry::{File, UNKNOWN};
    use std::collections::BTreeMap;

    const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/telemetry/");

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!("{DIR}{name}")).unwrap()
    }

    /// The real camera text; `/proc/stat` yields one line per read.
    struct Fixed {
        stat: Vec<String>,
    }
    impl Source for Fixed {
        fn read(&mut self, file: File) -> Option<String> {
            match file {
                File::Stat => (!self.stat.is_empty()).then(|| self.stat.remove(0)),
                File::Meminfo => Some(fixture("meminfo")),
                File::SelfStatus => Some(fixture("status")),
                File::Loadavg => Some(fixture("loadavg")),
                File::Uptime => Some(fixture("uptime")),
                File::Temperature => None,
            }
        }
        fn whole_degrees(&self) -> bool {
            true
        }
    }
    struct Link(Option<String>);
    impl Wifi for Link {
        fn iw_link(&mut self) -> Option<String> {
            self.0.clone()
        }
    }

    fn task(counters: &std::sync::Arc<StreamCounters>, link: Option<String>) -> Task {
        let stat = fixture("stat-10s");
        let source = Fixed {
            stat: stat.lines().map(str::to_owned).collect(),
        };
        Task::new(
            Collector::new(
                Box::new(source),
                Box::new(Link(link)),
                std::sync::Arc::clone(counters),
            ),
            Box::new(|| [Some(15), Some(10)]),
        )
    }

    #[test]
    fn the_wifi_parser_reads_the_real_camera_text() {
        assert_eq!(iw_signal_dbm(&fixture("iw-link")), Some(-53));
        assert_eq!(iw_signal_dbm("Not connected."), None);
        assert_eq!(
            iw_signal_dbm("signal: 47 dBm"),
            None,
            "positive is not a dBm reading"
        );
    }

    #[test]
    fn every_id_is_a_catalog_sensor_and_no_retired_id_survives() {
        assert_eq!(sensors().count(), IDS.len());
        for id in RETIRED {
            assert!(
                !IDS.contains(id) && !SENSORS.iter().any(|s| s.id == *id),
                "{id}"
            );
        }
        let secondary: Vec<_> = sensors()
            .filter(|s| !s.enabled_by_default)
            .map(|s| s.id)
            .collect();
        assert_eq!(
            secondary,
            [
                "daemon_memory",
                "load",
                "main_frames_short",
                "sub_fps_actual",
                "sub_bitrate_actual",
                "sub_frames_short"
            ]
        );
    }

    #[test]
    fn rates_come_from_the_counters_over_the_window_and_unknowns_are_unknown() {
        let counters = std::sync::Arc::new(StreamCounters::default());
        let mut t = task(&counters, Some(fixture("iw-link")));
        let first: BTreeMap<_, _> = t.round(Duration::ZERO, true).states.into_iter().collect();
        assert_eq!(first["cpu_busy"], UNKNOWN, "needs two samples");
        assert_eq!(first["temperature"], UNKNOWN, "unreadable is unknown");
        assert_eq!(first["main_fps_actual"], UNKNOWN, "no window yet");
        assert_eq!(first["memory_available"], "49.3");
        assert_eq!(first["wifi_signal"], "-53");
        assert_eq!(first.len(), IDS.len(), "the first round is complete");
        for _ in 0..450 {
            counters.frame(0, 8_533);
        }
        for _ in 0..270 {
            counters.frame(1, 6_400);
        }
        let s: BTreeMap<_, _> = t.round(INTERVAL, true).states.into_iter().collect();
        assert_eq!(s["main_fps_actual"], "15.0");
        assert_eq!(s["main_bitrate_actual"], "1024");
        assert_eq!(s["main_frames_short"], "0");
        assert_eq!(s["sub_fps_actual"], "9.0");
        assert_eq!(
            s["sub_frames_short"], "29",
            "300 expected, 270 delivered, one frame slack"
        );
        assert!(s.contains_key("cpu_busy"), "known now");
        assert!(
            !s.contains_key("memory_available") && !s.contains_key("wifi_signal"),
            "unchanged values stay quiet"
        );
    }

    #[test]
    fn a_reconnect_replays_every_sensor() {
        let counters = std::sync::Arc::new(StreamCounters::default());
        let mut t = task(&counters, None);
        assert_eq!(
            t.round(Duration::ZERO, false),
            Round::default(),
            "offline: nothing"
        );
        let first = t.round(INTERVAL, true);
        assert_eq!(first.states.len(), IDS.len());
        let quiet = t.round(INTERVAL * 2, true);
        assert!(
            !quiet.states.iter().any(|(id, _)| *id == "wifi_signal"),
            "still unknown: no republish"
        );
        assert!(t.round(INTERVAL * 3, false).states.is_empty());
        let again = t.round(INTERVAL * 4, true);
        assert_eq!(again.states.len(), IDS.len(), "the filter starts afresh");
    }
}
