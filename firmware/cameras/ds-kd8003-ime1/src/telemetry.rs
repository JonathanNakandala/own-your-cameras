//! The gate's diagnostics on the shared telemetry layer: which catalog sensors it has, their
//! entities, the stream rates counted from the encoder loops, and the ISP read-outs. A round every
//! [`SAMPLE_EVERY`] goes through the shared change filter, so only moved values reach the broker.
use camera_transport::mqtt::{Category, Domain, Entity};
use camera_transport::telemetry::{Reporter, SAMPLE_EVERY, Sampler, Sensor, Source, sensor_by_id};
use std::time::Duration;

/// The catalog sensors the gate has a source for (no temperature sensor, no Wi-Fi).
pub const IDS: &[&str] = &[
    "cpu_busy",
    "memory_available",
    "daemon_memory",
    "load",
    "uptime",
    "main_fps_actual",
    "main_bitrate_actual",
    "sub_fps_actual",
    "sub_bitrate_actual",
    "isp_luma",
    "isp_night",
];

/// ISP keys read with `ISP_GetKeyParam`.
pub const KEY_LUMA: u32 = 34;
// Key 42 (total gain dB) is not read: on V2.2.62 its getter only prints the value.
pub const KEY_NIGHT: u32 = 45;

pub fn sensors() -> Vec<Sensor> {
    IDS.iter()
        .map(|id| *sensor_by_id(id).expect("catalog sensor"))
        .collect()
}

/// The Home Assistant entity of a catalog sensor: diagnostic, `state_class: measurement` (so
/// `None` reads as unknown), disabled by default where the catalog says so.
pub fn entity(s: &Sensor) -> Entity {
    let mut e = Entity::new(s.id, s.name, Domain::Sensor)
        .category(Category::Diagnostic)
        .state_class("measurement");
    if let Some(unit) = s.unit {
        e = e.unit(unit);
    }
    if let Some(class) = s.device_class {
        e = e.device_class(class);
    }
    if let Some(icon) = s.icon {
        e = e.icon(icon);
    }
    if !s.enabled_by_default {
        e = e.disabled_by_default();
    }
    e
}

/// Frames and bytes each encoder loop delivered since start.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    pub frames: [u64; 2],
    pub bytes: [u64; 2],
}

/// One round's ISP read-outs, raw; `None` when the key did not read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Isp {
    pub luma: Option<u32>,
    pub night: Option<u32>,
}

/// Samples and filters; one per process.
pub struct Telemetry<S: Source> {
    sampler: Sampler<S>,
    reporter: Reporter,
    last: Option<(Counters, Duration)>,
    connected: bool,
}

impl<S: Source> Telemetry<S> {
    pub fn new(source: S) -> Self {
        Self {
            sampler: Sampler::new(source),
            reporter: Reporter::new(&sensors()),
            last: None,
            connected: false,
        }
    }

    /// `(fps, kbit/s)` per stream since the previous round; unknown on the first one or when a
    /// counter went backwards.
    fn rates(&mut self, now_counters: Counters, now: Duration) -> [(Option<f64>, Option<f64>); 2] {
        let previous = self.last.replace((now_counters, now));
        let mut out = [(None, None); 2];
        let Some((before, at)) = previous else {
            return out;
        };
        let secs = now.saturating_sub(at).as_secs_f64();
        if secs <= 0.0 {
            return out;
        }
        for (chn, slot) in out.iter_mut().enumerate() {
            let frames = now_counters.frames[chn].checked_sub(before.frames[chn]);
            let bytes = now_counters.bytes[chn].checked_sub(before.bytes[chn]);
            *slot = (
                frames.map(|f| f as f64 / secs),
                bytes.map(|b| b as f64 * 8.0 / 1000.0 / secs),
            );
        }
        out
    }

    /// One round at `now`: the states worth publishing. A reconnect makes the round complete, so
    /// the broker's retained values are fresh again.
    pub fn round(
        &mut self,
        now: Duration,
        counters: Counters,
        isp: Isp,
        connected: bool,
    ) -> Vec<(&'static str, String)> {
        if connected && !self.connected {
            self.reporter.reset();
        }
        self.connected = connected;
        let readings = self.sampler.sample();
        let [(main_fps, main_kbps), (sub_fps, sub_kbps)] = self.rates(counters, now);
        let mut entries: Vec<(&'static str, Option<f64>)> = readings
            .entries()
            .into_iter()
            .filter(|(id, _)| IDS.contains(id))
            .collect();
        entries.extend([
            ("main_fps_actual", main_fps),
            ("main_bitrate_actual", main_kbps),
            ("sub_fps_actual", sub_fps),
            ("sub_bitrate_actual", sub_kbps),
            ("isp_luma", isp.luma.map(f64::from)),
            ("isp_night", isp.night.map(|n| f64::from(u8::from(n != 0)))),
        ]);
        if !connected {
            // Nothing reaches the broker; keep the filter's memory for the reconnect reset.
            return Vec::new();
        }
        entries
            .into_iter()
            .filter_map(|(id, value)| Some((id, self.reporter.offer(id, value, now)?)))
            .collect()
    }
}

/// How often the runtime calls [`Telemetry::round`].
pub const EVERY: Duration = SAMPLE_EVERY;

#[cfg(test)]
mod tests {
    use super::*;
    use camera_transport::telemetry::{File, UNKNOWN};

    struct Fixture(u64);
    impl Source for Fixture {
        fn read(&mut self, file: File) -> Option<String> {
            self.0 += 1;
            Some(match file {
                File::Stat => format!("cpu  {} 0 0 {} 0 0 0 0 0 0\n", self.0 * 10, self.0 * 30),
                File::Meminfo => "MemTotal: 60000 kB\nMemAvailable: 20480 kB\n".into(),
                File::SelfStatus => "VmRSS:\t   10240 kB\n".into(),
                File::Loadavg => "0.50 0.40 0.30 1/50 99\n".into(),
                File::Uptime => "1234.5 100.0\n".into(),
                _ => return None,
            })
        }
    }

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    #[test]
    fn every_sensor_is_a_diagnostic_measurement_and_secondary_ones_start_disabled() {
        for s in sensors() {
            let e = entity(&s);
            assert_eq!(e.category, Some(Category::Diagnostic));
            assert_eq!(e.state_class, Some("measurement"));
            assert_eq!(e.enabled_by_default, s.enabled_by_default, "{}", s.id);
        }
        let disabled: Vec<&str> = sensors()
            .iter()
            .filter(|s| !s.enabled_by_default)
            .map(|s| s.id)
            .collect();
        for id in [
            "load",
            "daemon_memory",
            "sub_fps_actual",
            "sub_bitrate_actual",
            "isp_luma",
            "isp_night",
        ] {
            assert!(disabled.contains(&id), "{id}");
        }
    }

    #[test]
    fn rounds_publish_changes_only_and_a_reconnect_republishes_everything() {
        let mut t = Telemetry::new(Fixture(0));
        let isp = Isp {
            luma: Some(120),
            night: Some(0),
        };
        let c0 = Counters::default();
        let first = t.round(secs(0), c0, isp, true);
        assert_eq!(first.len(), IDS.len(), "the first round is complete");
        let fps = first
            .iter()
            .find(|(id, _)| *id == "main_fps_actual")
            .unwrap();
        assert_eq!(fps.1, UNKNOWN, "no rate before two counts");

        let c1 = Counters {
            frames: [900, 450],
            bytes: [15_360_000, 3_840_000],
        };
        let second = t.round(secs(30), c1, isp, true);
        let get = |r: &[(&str, String)], id: &str| {
            r.iter().find(|(i, _)| *i == id).map(|(_, v)| v.clone())
        };
        assert_eq!(get(&second, "main_fps_actual").as_deref(), Some("30.0"));
        assert_eq!(get(&second, "main_bitrate_actual").as_deref(), Some("4096"));
        assert_eq!(get(&second, "sub_fps_actual").as_deref(), Some("15.0"));
        assert_eq!(get(&second, "isp_luma"), None, "inside the deadband");
        assert_eq!(get(&second, "memory_available"), None);

        let c2 = Counters {
            frames: [1800, 900],
            bytes: [30_720_000, 7_680_000],
        };
        let quiet = t.round(secs(60), c2, isp, true);
        assert!(
            quiet
                .iter()
                .all(|(id, _)| *id == "cpu_busy" || *id == "uptime"),
            "{quiet:?}"
        );

        assert!(t.round(secs(90), c2, isp, false).is_empty());
        let back = t.round(secs(120), c2, isp, true);
        assert_eq!(back.len(), IDS.len());
    }

    #[test]
    fn missing_isp_reads_are_unknown_not_zero() {
        let mut t = Telemetry::new(Fixture(0));
        let r = t.round(secs(0), Counters::default(), Isp::default(), true);
        for id in ["isp_luma", "isp_night"] {
            assert_eq!(r.iter().find(|(i, _)| *i == id).unwrap().1, UNKNOWN);
        }
    }
}
