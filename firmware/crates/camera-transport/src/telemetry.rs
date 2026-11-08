//! Diagnostic telemetry shared by every camera: the sensor catalog, `/proc` readers behind a
//! [`Source`], and the change filter that decides what is worth publishing.
//!
//! A camera samples every [`SAMPLE_EVERY`] and offers each reading to a [`Reporter`]; only the
//! payloads it returns go to `Publisher::state`. A value is published when it moves beyond its
//! sensor's deadband from the last published value, when [`MAX_INTERVAL`] passed since then, or
//! when it turns unknown or known. A missing reading is unknown ([`UNKNOWN`]), never zero.
//! [`Reporter::reset`] after a reconnect makes the next round complete.

use std::collections::BTreeMap;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::time::Duration;

/// How often a camera samples its sensors.
pub const SAMPLE_EVERY: Duration = Duration::from_secs(30);
/// A sensor is republished at least this often, changed or not.
pub const MAX_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// The state payload of an unknown reading: Home Assistant's MQTT sensors read `None` as unknown.
/// HA reads it so only for a numeric sensor, so discovery must give every catalog sensor a unit, a
/// numeric device class or `state_class: measurement`. An empty payload is not unknown: HA ignores
/// it and keeps the old value.
pub const UNKNOWN: &str = "None";

/// How far a value must move from the last published one to be published again.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Deadband {
    /// In the sensor's unit.
    Absolute(f64),
    /// A share of the last published value (0.1 = 10 %).
    Relative(f64),
}

/// One diagnostic sensor of the shared catalog. Every sensor is `entity_category: diagnostic`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sensor {
    /// Unique id, the same on every camera.
    pub id: &'static str,
    pub name: &'static str,
    pub unit: Option<&'static str>,
    pub device_class: Option<&'static str>,
    pub icon: Option<&'static str>,
    pub deadband: Deadband,
    /// Decimals in the state payload.
    pub decimals: u8,
    pub enabled_by_default: bool,
}

const fn sensor(
    id: &'static str,
    name: &'static str,
    unit: Option<&'static str>,
    device_class: Option<&'static str>,
    deadband: Deadband,
    decimals: u8,
    enabled_by_default: bool,
) -> Sensor {
    Sensor {
        id,
        name,
        unit,
        device_class,
        icon: None,
        deadband,
        decimals,
        enabled_by_default,
    }
}

const fn icon(mut s: Sensor, icon: &'static str) -> Sensor {
    s.icon = Some(icon);
    s
}

use Deadband::{Absolute, Relative};

/// Every telemetry sensor any camera publishes; a camera uses the subset it has a source for.
pub const SENSORS: &[Sensor] = &[
    icon(
        sensor(
            "cpu_busy",
            "CPU busy",
            Some("%"),
            None,
            Absolute(5.0),
            0,
            true,
        ),
        "mdi:cpu-32-bit",
    ),
    sensor(
        "memory_available",
        "Memory available",
        Some("MiB"),
        Some("data_size"),
        Absolute(1.0),
        1,
        true,
    ),
    sensor(
        "daemon_memory",
        "Daemon memory",
        Some("MiB"),
        Some("data_size"),
        Absolute(1.0),
        1,
        false,
    ),
    icon(
        sensor("load", "Load average", None, None, Absolute(0.5), 2, false),
        "mdi:gauge",
    ),
    // Grows steadily: the max interval alone paces it.
    sensor(
        "uptime",
        "Uptime",
        Some("s"),
        Some("duration"),
        Absolute(f64::INFINITY),
        0,
        true,
    ),
    sensor(
        "temperature",
        "Temperature",
        Some("°C"),
        Some("temperature"),
        Absolute(1.0),
        0,
        true,
    ),
    icon(
        sensor(
            "main_fps_actual",
            "Main frame rate",
            Some("fps"),
            None,
            Absolute(1.0),
            1,
            true,
        ),
        "mdi:filmstrip",
    ),
    sensor(
        "main_bitrate_actual",
        "Main bitrate",
        Some("kbit/s"),
        Some("data_rate"),
        Relative(0.1),
        0,
        true,
    ),
    icon(
        sensor(
            "sub_fps_actual",
            "Sub frame rate",
            Some("fps"),
            None,
            Absolute(1.0),
            1,
            false,
        ),
        "mdi:filmstrip",
    ),
    sensor(
        "sub_bitrate_actual",
        "Sub bitrate",
        Some("kbit/s"),
        Some("data_rate"),
        Relative(0.1),
        0,
        false,
    ),
    // Frames an encoder delivered short of its configured rate over a sampling window (Reolink).
    icon(
        sensor(
            "main_frames_short",
            "Main frames short",
            None,
            None,
            Absolute(3.0),
            0,
            false,
        ),
        "mdi:filmstrip-off",
    ),
    icon(
        sensor(
            "sub_frames_short",
            "Sub frames short",
            None,
            None,
            Absolute(3.0),
            0,
            false,
        ),
        "mdi:filmstrip-off",
    ),
    sensor(
        "wifi_signal",
        "Wi-Fi signal",
        Some("dBm"),
        Some("signal_strength"),
        Absolute(3.0),
        0,
        true,
    ),
    icon(
        sensor(
            "isp_luma",
            "Scene luma",
            None,
            None,
            Absolute(8.0),
            0,
            false,
        ),
        "mdi:brightness-6",
    ),
    sensor(
        "isp_gain_db",
        "Sensor gain",
        Some("dB"),
        None,
        Absolute(1.0),
        1,
        false,
    ),
    // Exposure time of the auto-exposure loop (Mi 360); it moves in steps, so 10 % is a change.
    icon(
        sensor(
            "isp_exposure_us",
            "Exposure time",
            Some("µs"),
            None,
            Relative(0.1),
            0,
            false,
        ),
        "mdi:camera-timer",
    ),
    icon(
        sensor(
            "isp_night",
            "ISP night mode",
            None,
            None,
            Absolute(0.5),
            0,
            false,
        ),
        "mdi:weather-night",
    ),
];

impl Sensor {
    /// The Home Assistant sensor: diagnostic, `state_class: measurement` so that HA reads
    /// [`UNKNOWN`] as unknown, disabled by default when secondary.
    pub fn entity(&self) -> crate::mqtt::Entity {
        use crate::mqtt::{Category, Domain, Entity};
        let mut e = Entity::new(self.id, self.name, Domain::Sensor)
            .category(Category::Diagnostic)
            .state_class("measurement");
        if let Some(unit) = self.unit {
            e = e.unit(unit);
        }
        if let Some(class) = self.device_class {
            e = e.device_class(class);
        }
        if let Some(icon) = self.icon {
            e = e.icon(icon);
        }
        if !self.enabled_by_default {
            e = e.disabled_by_default();
        }
        e
    }
}

pub fn sensor_by_id(id: &str) -> Option<&'static Sensor> {
    SENSORS.iter().find(|s| s.id == id)
}

/// The publication rule of one sensor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gauge {
    pub deadband: Deadband,
    pub max_interval: Duration,
}

impl Gauge {
    /// Whether `next` is published after `last` was published `since` ago.
    pub fn publishes(&self, last: Option<f64>, next: Option<f64>, since: Duration) -> bool {
        if since >= self.max_interval {
            return true;
        }
        match (last, next) {
            (Some(a), Some(b)) => {
                let moved = (b - a).abs();
                let band = match self.deadband {
                    Absolute(d) => d,
                    Relative(share) => share * a.abs(),
                };
                moved > 0.0 && moved >= band
            }
            (None, None) => false,
            _ => true,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Published {
    value: Option<f64>,
    at: Duration,
}

/// The time-injected change filter in front of `Publisher::state`.
#[derive(Debug)]
pub struct Reporter {
    sensors: BTreeMap<&'static str, (Gauge, u8)>,
    last: BTreeMap<&'static str, Published>,
}

impl Reporter {
    /// For the given sensors, each with its catalog deadband and [`MAX_INTERVAL`].
    pub fn new(sensors: &[Sensor]) -> Self {
        Self {
            sensors: sensors
                .iter()
                .map(|s| {
                    let gauge = Gauge {
                        deadband: s.deadband,
                        max_interval: MAX_INTERVAL,
                    };
                    (s.id, (gauge, s.decimals))
                })
                .collect(),
            last: BTreeMap::new(),
        }
    }

    /// Offers a reading at `now` (any monotonic time). Returns the state payload to publish, or
    /// `None` to stay quiet. A non-finite reading is unknown; an id this reporter lacks is never
    /// published.
    pub fn offer(&mut self, id: &str, value: Option<f64>, now: Duration) -> Option<String> {
        let (&id, &(gauge, decimals)) = self.sensors.get_key_value(id)?;
        let value = value.filter(|v| v.is_finite());
        if let Some(last) = self.last.get(id)
            && !gauge.publishes(last.value, value, now.saturating_sub(last.at))
        {
            return None;
        }
        self.last.insert(id, Published { value, at: now });
        Some(match value {
            Some(v) => format!("{v:.*}", usize::from(decimals)),
            None => UNKNOWN.into(),
        })
    }

    /// Forgets what was published, so every next offer publishes (after a reconnect).
    pub fn reset(&mut self) {
        self.last.clear();
    }
}

/// The system files telemetry reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum File {
    /// `/proc/stat`
    Stat,
    /// `/proc/meminfo`
    Meminfo,
    /// `/proc/self/status`
    SelfStatus,
    /// `/proc/loadavg`
    Loadavg,
    /// `/proc/uptime`
    Uptime,
    /// The camera's thermal sensor, if it has one.
    Temperature,
}

/// Where the readings come from: the real system or a test fixture.
pub trait Source: Send {
    /// The file's text; `None` if it is absent or unreadable.
    fn read(&mut self, file: File) -> Option<String>;
    /// The unit of [`File::Temperature`]; millidegrees unless the source says otherwise.
    fn whole_degrees(&self) -> bool {
        false
    }
}

impl Source for Box<dyn Source> {
    fn read(&mut self, file: File) -> Option<String> {
        (**self).read(file)
    }
    fn whole_degrees(&self) -> bool {
        (**self).whole_degrees()
    }
}

/// How a thermal file states its temperature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Thermal {
    /// Linux thermal zones: millidegrees Celsius.
    Millidegrees(&'static str),
    /// Some vendor drivers (Novatek `nvt_thermal`): whole degrees.
    Degrees(&'static str),
}

/// Reads `/proc`, and the thermal file the camera names, bounded and never through a final symlink
/// or blocking on a FIFO.
#[derive(Debug, Clone, Default)]
pub struct Proc {
    /// e.g. `Thermal::Millidegrees("/sys/class/thermal/thermal_zone0/temp")`; `None` when the
    /// camera has no sensor.
    pub temperature: Option<Thermal>,
}

const MAX_READ: u64 = 64 * 1024;

impl Source for Proc {
    fn whole_degrees(&self) -> bool {
        matches!(self.temperature, Some(Thermal::Degrees(_)))
    }

    fn read(&mut self, file: File) -> Option<String> {
        let path = match file {
            File::Stat => "/proc/stat",
            File::Meminfo => "/proc/meminfo",
            File::SelfStatus => "/proc/self/status",
            File::Loadavg => "/proc/loadavg",
            File::Uptime => "/proc/uptime",
            File::Temperature => match self.temperature? {
                Thermal::Millidegrees(p) | Thermal::Degrees(p) => p,
            },
        };
        let f = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(path)
            .ok()?;
        if !f.metadata().ok()?.is_file() {
            return None;
        }
        let mut text = String::new();
        f.take(MAX_READ).read_to_string(&mut text).ok()?;
        Some(text)
    }
}

/// One round of system readings; `None` is unknown.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Readings {
    pub cpu_busy: Option<f64>,
    pub memory_available: Option<f64>,
    pub daemon_memory: Option<f64>,
    pub load: Option<f64>,
    pub uptime: Option<f64>,
    pub temperature: Option<f64>,
}

impl Readings {
    /// `(sensor id, reading)` for [`Reporter::offer`].
    pub fn entries(&self) -> [(&'static str, Option<f64>); 6] {
        [
            ("cpu_busy", self.cpu_busy),
            ("memory_available", self.memory_available),
            ("daemon_memory", self.daemon_memory),
            ("load", self.load),
            ("uptime", self.uptime),
            ("temperature", self.temperature),
        ]
    }
}

/// Turns successive reads of a [`Source`] into [`Readings`]; CPU busy needs two reads.
#[derive(Debug)]
pub struct Sampler<S: Source> {
    source: S,
    cpu: Option<(u64, u64)>,
}

impl<S: Source> Sampler<S> {
    pub fn new(source: S) -> Self {
        Self { source, cpu: None }
    }

    pub fn sample(&mut self) -> Readings {
        let mut read = |f| self.source.read(f);
        let stat = read(File::Stat).as_deref().and_then(total_idle);
        let mem = read(File::Meminfo);
        let status = read(File::SelfStatus);
        let load = read(File::Loadavg);
        let uptime = read(File::Uptime);
        let temperature = read(File::Temperature);
        let cpu_busy = match (self.cpu, stat) {
            (Some(before), Some(now)) => busy_percent(before, now),
            _ => None,
        };
        // An unparsable read keeps the previous baseline.
        if stat.is_some() {
            self.cpu = stat;
        }
        Readings {
            cpu_busy,
            memory_available: mem.as_deref().and_then(available_kib).map(mib),
            daemon_memory: status.as_deref().and_then(|t| kib(t, "VmRSS:")).map(mib),
            load: load.as_deref().and_then(first_number),
            uptime: uptime.as_deref().and_then(first_number),
            temperature: temperature
                .as_deref()
                .and_then(|t| celsius(t, self.source.whole_degrees())),
        }
    }
}

/// `(total, idle)` jiffies of the aggregate `cpu` line; idle includes iowait, and guest time is
/// already part of user time.
fn total_idle(stat: &str) -> Option<(u64, u64)> {
    let line = stat.lines().find(|l| l.starts_with("cpu "))?;
    let f: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .take(8)
        .map(|v| v.parse().ok())
        .collect::<Option<_>>()?;
    if f.len() < 5 {
        return None;
    }
    let total = f.iter().try_fold(0u64, |a, n| a.checked_add(*n))?;
    Some((total, f[3].checked_add(f[4])?))
}

/// Busy share between two reads; unknown when no time passed or the counters went backwards.
fn busy_percent(before: (u64, u64), now: (u64, u64)) -> Option<f64> {
    let total = now.0.checked_sub(before.0)?;
    let idle = now.1.checked_sub(before.1)?;
    (total > 0 && idle <= total).then(|| (total - idle) as f64 * 100.0 / total as f64)
}

/// A `Name:   1234 kB` field.
fn kib(text: &str, name: &str) -> Option<u64> {
    let mut f = text
        .lines()
        .find_map(|l| l.strip_prefix(name))?
        .split_whitespace();
    let n = f.next()?.parse().ok()?;
    (f.next() == Some("kB") && f.next().is_none()).then_some(n)
}

/// `MemAvailable`, or on kernels before 3.14, which lack it, the usual estimate
/// `MemFree + Buffers + Cached`. The estimate reads higher than MemAvailable would (it counts
/// shmem and cache that cannot be dropped). A garbled `MemAvailable` line is unknown, not estimated.
fn available_kib(meminfo: &str) -> Option<u64> {
    if meminfo.lines().any(|l| l.starts_with("MemAvailable:")) {
        return kib(meminfo, "MemAvailable:");
    }
    ["MemFree:", "Buffers:", "Cached:"]
        .into_iter()
        .try_fold(0u64, |sum, name| sum.checked_add(kib(meminfo, name)?))
}

fn mib(kib: u64) -> f64 {
    kib as f64 / 1024.0
}

fn first_number(text: &str) -> Option<f64> {
    text.split_whitespace()
        .next()?
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v >= 0.0)
}

fn celsius(text: &str, whole_degrees: bool) -> Option<f64> {
    let n: i64 = text.trim().parse().ok()?;
    let c = if whole_degrees {
        n as f64
    } else {
        n as f64 / 1000.0
    };
    (-60.0..=200.0).contains(&c).then_some(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/telemetry/");

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!("{DIR}{name}")).unwrap()
    }

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    /// Serves the real camera text; `/proc/stat` yields one line per read.
    struct Fixture {
        stat: Vec<String>,
        temperature: Option<String>,
    }
    impl Fixture {
        fn new() -> Self {
            Self {
                stat: fixture("stat-10s").lines().map(str::to_owned).collect(),
                temperature: Some(fixture("thermal_zone0_temp")),
            }
        }
    }
    impl Source for Fixture {
        fn read(&mut self, file: File) -> Option<String> {
            match file {
                File::Stat => (!self.stat.is_empty()).then(|| self.stat.remove(0)),
                File::Meminfo => Some(fixture("meminfo")),
                File::SelfStatus => Some(fixture("status")),
                File::Loadavg => Some(fixture("loadavg")),
                File::Uptime => Some(fixture("uptime")),
                File::Temperature => self.temperature.clone(),
            }
        }
        fn whole_degrees(&self) -> bool {
            true
        }
    }

    #[test]
    fn the_catalog_has_unique_ids_and_the_ticket_deadbands() {
        for (i, s) in SENSORS.iter().enumerate() {
            assert!(SENSORS[..i].iter().all(|o| o.id != s.id), "{}", s.id);
        }
        let band = |id| sensor_by_id(id).unwrap().deadband;
        assert_eq!(band("cpu_busy"), Absolute(5.0));
        assert_eq!(band("memory_available"), Absolute(1.0));
        assert_eq!(band("temperature"), Absolute(1.0));
        assert_eq!(band("main_fps_actual"), Absolute(1.0));
        assert_eq!(band("main_bitrate_actual"), Relative(0.1));
        let secondary: Vec<_> = SENSORS
            .iter()
            .filter(|s| !s.enabled_by_default)
            .map(|s| s.id)
            .collect();
        assert_eq!(
            secondary,
            [
                "daemon_memory",
                "load",
                "sub_fps_actual",
                "sub_bitrate_actual",
                "main_frames_short",
                "sub_frames_short",
                "isp_luma",
                "isp_gain_db",
                "isp_exposure_us",
                "isp_night"
            ]
        );
    }

    #[test]
    fn every_sensor_entity_is_numeric_diagnostic_and_secondary_ones_start_disabled() {
        use crate::mqtt::{Category, Domain};
        for s in SENSORS {
            let e = s.entity();
            assert_eq!((e.id.as_str(), e.domain.clone()), (s.id, Domain::Sensor));
            assert_eq!(e.category, Some(Category::Diagnostic));
            assert_eq!(
                e.state_class,
                Some("measurement"),
                "{}: unknown needs it",
                s.id
            );
            assert_eq!(e.enabled_by_default, s.enabled_by_default);
            assert_eq!(
                (e.unit, e.device_class, e.icon),
                (s.unit, s.device_class, s.icon)
            );
        }
    }

    #[test]
    fn readers_parse_the_real_camera_text() {
        let mut s = Sampler::new(Fixture::new());
        let first = s.sample();
        assert_eq!(first.cpu_busy, None, "needs two reads");
        assert_eq!(first.memory_available, Some(50456.0 / 1024.0));
        assert_eq!(first.daemon_memory, Some(3320.0 / 1024.0));
        assert_eq!(first.load, Some(32.75));
        assert_eq!(first.uptime, Some(3703.64));
        assert_eq!(first.temperature, Some(66.0));
        // 10 s apart: total +977, idle (idle + iowait) +724.
        let busy = s.sample().cpu_busy.unwrap();
        assert!((busy - 253.0 * 100.0 / 977.0).abs() < 1e-9, "{busy}");
    }

    #[test]
    fn an_old_kernel_without_mem_available_gets_free_plus_buffers_plus_cached() {
        // Linux 3.10 (Mi 360): no MemAvailable line.
        let old = fixture("meminfo-3.10");
        assert!(!old.contains("MemAvailable"));
        assert_eq!(available_kib(&old), Some(26824 + 6432 + 19488));
        assert_eq!(
            available_kib(&fixture("meminfo")),
            Some(50456),
            "never estimated"
        );
        assert_eq!(
            available_kib("MemAvailable: lots kB\nMemFree: 1 kB\nBuffers: 1 kB\nCached: 1 kB"),
            None,
            "a garbled MemAvailable is unknown"
        );
        assert_eq!(
            available_kib("MemFree: 1 kB\nCached: 1 kB"),
            None,
            "all three needed"
        );
        struct Old;
        impl Source for Old {
            fn read(&mut self, file: File) -> Option<String> {
                (file == File::Meminfo).then(|| fixture("meminfo-3.10"))
            }
        }
        let r = Sampler::new(Old).sample();
        assert_eq!(r.memory_available, Some(52744.0 / 1024.0));
    }

    #[test]
    fn a_missing_or_garbled_reading_is_unknown_never_zero() {
        let mut s = Sampler::new(Fixture {
            stat: vec![],
            temperature: None,
        });
        let r = s.sample();
        assert_eq!((r.cpu_busy, r.temperature), (None, None));
        assert_eq!(kib("MemAvailable: lots kB", "MemAvailable:"), None);
        assert_eq!(kib("MemAvailable: 12 MB", "MemAvailable:"), None);
        assert_eq!(kib("", "VmRSS:"), None);
        assert_eq!(first_number("nan 1 2"), None);
        assert_eq!(first_number(""), None);
        assert_eq!(celsius("hot", false), None);
        assert_eq!(celsius("45000\n", false), Some(45.0), "millidegrees");
        assert_eq!(
            celsius("150", false),
            Some(0.15),
            "a cold millidegree reading"
        );
        assert_eq!(celsius("66\n", true), Some(66.0), "whole degrees");
        assert_eq!(celsius("-273000", false), None);
        assert_eq!(total_idle("cpu  1 2 3"), None);
        assert_eq!(total_idle("cpu0 1 2 3 4 5"), None, "aggregate line only");
    }

    #[test]
    fn cpu_baseline_survives_a_bad_read_and_a_still_or_backwards_clock_is_unknown() {
        let lines = [
            "cpu  0 0 0 100 0 0 0 0",
            "garbage",
            "cpu  50 0 0 150 0 0 0 0",
        ];
        let mut s = Sampler::new(Fixture {
            stat: lines.map(str::to_owned).to_vec(),
            temperature: None,
        });
        assert_eq!(s.sample().cpu_busy, None);
        assert_eq!(s.sample().cpu_busy, None);
        assert_eq!(s.sample().cpu_busy, Some(50.0));
        assert_eq!(busy_percent((10, 5), (10, 5)), None);
        assert_eq!(busy_percent((10, 5), (9, 5)), None);
    }

    #[test]
    fn readings_map_onto_catalog_ids() {
        for (id, _) in Readings::default().entries() {
            assert!(sensor_by_id(id).is_some(), "{id}");
        }
    }

    fn reporter() -> Reporter {
        Reporter::new(SENSORS)
    }

    #[test]
    fn the_first_offer_publishes_and_values_inside_the_deadband_stay_quiet() {
        let mut r = reporter();
        assert_eq!(r.offer("cpu_busy", Some(20.4), secs(0)), Some("20".into()));
        assert_eq!(r.offer("cpu_busy", Some(24.9), secs(30)), None);
        assert_eq!(r.offer("cpu_busy", Some(16.0), secs(60)), None);
        assert_eq!(r.offer("cpu_busy", Some(25.5), secs(90)), Some("26".into()));
    }

    #[test]
    fn the_deadband_is_measured_from_the_last_published_value() {
        let mut r = reporter();
        r.offer("temperature", Some(50.0), secs(0));
        assert_eq!(r.offer("temperature", Some(50.6), secs(30)), None);
        assert_eq!(r.offer("temperature", Some(50.9), secs(60)), None);
        assert_eq!(
            r.offer("temperature", Some(51.0), secs(90)),
            Some("51".into())
        );
    }

    #[test]
    fn a_relative_deadband_scales_with_the_value() {
        let mut r = reporter();
        r.offer("main_bitrate_actual", Some(2000.0), secs(0));
        assert_eq!(r.offer("main_bitrate_actual", Some(2150.0), secs(30)), None);
        assert_eq!(
            r.offer("main_bitrate_actual", Some(1800.0), secs(60)),
            Some("1800".into())
        );
        r.offer("sub_bitrate_actual", Some(0.0), secs(0));
        assert_eq!(
            r.offer("sub_bitrate_actual", Some(1.0), secs(30)),
            Some("1".into()),
            "any change from zero"
        );
    }

    #[test]
    fn an_unchanged_value_is_republished_after_the_max_interval() {
        let mut r = reporter();
        r.offer("memory_available", Some(49.3), secs(0));
        assert_eq!(r.offer("memory_available", Some(49.3), secs(270)), None);
        assert_eq!(
            r.offer("memory_available", Some(49.3), secs(300)),
            Some("49.3".into())
        );
        assert_eq!(r.offer("memory_available", Some(49.3), secs(330)), None);
        r.offer("uptime", Some(3703.6), secs(0));
        assert_eq!(
            r.offer("uptime", Some(3733.6), secs(30)),
            None,
            "paced by the interval"
        );
        assert_eq!(
            r.offer("uptime", Some(4003.6), secs(300)),
            Some("4004".into())
        );
    }

    #[test]
    fn unknown_and_known_always_publish_and_unknown_is_never_zero() {
        let mut r = reporter();
        r.offer("cpu_busy", Some(20.0), secs(0));
        assert_eq!(r.offer("cpu_busy", None, secs(30)), Some(UNKNOWN.into()));
        assert_eq!(r.offer("cpu_busy", None, secs(60)), None);
        assert_eq!(
            r.offer("cpu_busy", Some(f64::NAN), secs(90)),
            None,
            "NaN is unknown"
        );
        assert_eq!(
            r.offer("cpu_busy", Some(20.0), secs(120)),
            Some("20".into())
        );
        assert_eq!(
            r.offer("load", None, secs(0)),
            Some(UNKNOWN.into()),
            "first offer"
        );
        assert_eq!(
            r.offer("load", None, secs(300)),
            Some(UNKNOWN.into()),
            "max interval"
        );
    }

    #[test]
    fn reset_makes_the_next_round_complete_and_foreign_ids_are_never_published() {
        let mut r = Reporter::new(&SENSORS[..2]);
        r.offer("cpu_busy", Some(20.0), secs(0));
        r.offer("memory_available", Some(49.0), secs(0));
        assert_eq!(r.offer("cpu_busy", Some(20.0), secs(30)), None);
        r.reset();
        assert_eq!(r.offer("cpu_busy", Some(20.0), secs(31)), Some("20".into()));
        assert_eq!(
            r.offer("memory_available", Some(49.0), secs(31)),
            Some("49.0".into())
        );
        assert_eq!(
            r.offer("load", Some(1.0), secs(0)),
            None,
            "not this camera's sensor"
        );
    }

    #[test]
    fn the_real_proc_source_reads_this_host_where_it_exists() {
        // Linux only; elsewhere every read is unknown, which is the rule too.
        let r = Sampler::new(Proc::default()).sample();
        if std::path::Path::new("/proc/meminfo").exists() {
            assert!(r.memory_available.is_some());
        } else {
            assert_eq!(r, Readings::default());
        }
    }
}
