//! Diagnostic telemetry on the shared sensor catalog: sampled every 30 s, published only through
//! the change filter. Wi-Fi is read here, because its `/proc/net/wireless` fields are driver-specific.
use super::*;
use camera_transport::telemetry::{Proc, Reporter, SAMPLE_EVERY, Sampler};

/// Samples system, Wi-Fi and stream rates and offers them to the reporter.
pub(super) struct Telemetry {
    sampler: Sampler<Proc>,
    reporter: Reporter,
    connected: bool,
    /// Frames and bytes per stream (main, sub) since the last sample.
    counts: [u64; 2],
    bytes: [u64; 2],
    since: Instant,
}
impl Telemetry {
    pub(super) fn new(now: Instant) -> Self {
        let mut sampler = Sampler::new(Proc::default());
        // CPU busy needs a baseline; the first round then has a real value.
        sampler.sample();
        Self {
            sampler,
            reporter: Reporter::new(
                &crate::catalog::telemetry()
                    .chain(crate::catalog::isp_telemetry().map(|(_, s)| s))
                    .copied()
                    .collect::<Vec<_>>(),
            ),
            connected: false,
            counts: [0; 2],
            bytes: [0; 2],
            since: now,
        }
    }
    pub(super) fn frame(&mut self, channel: usize, len: usize) {
        self.counts[channel] += 1;
        self.bytes[channel] += len as u64;
    }
    /// Once a second. `uptime` is the monotonic time the reporter runs on; `isp` reads the ISP
    /// sensors, only when a round is due.
    pub(super) fn tick(
        &mut self,
        publisher: &mqtt::Publisher,
        now: Instant,
        uptime: Duration,
        isp: impl FnOnce() -> Vec<(&'static str, Option<f64>)>,
    ) {
        let connected = publisher.is_connected();
        if connected && !self.connected {
            // A reconnect: the next round is complete. Retired ids are cleared by every
            // announcement (`Config::retired`).
            self.reporter.reset();
        }
        self.connected = connected;
        let elapsed = now.saturating_duration_since(self.since);
        if elapsed < SAMPLE_EVERY {
            return;
        }
        let seconds = elapsed.as_secs_f64();
        let rate = |n: u64| Some(n as f64 / seconds);
        let readings = self.sampler.sample().entries().into_iter().chain([
            ("main_fps_actual", rate(self.counts[0])),
            ("sub_fps_actual", rate(self.counts[1])),
            (
                "main_bitrate_actual",
                rate(self.bytes[0] * 8).map(|b| b / 1000.0),
            ),
            (
                "sub_bitrate_actual",
                rate(self.bytes[1] * 8).map(|b| b / 1000.0),
            ),
            (
                "wifi_signal",
                std::fs::read_to_string("/proc/net/wireless")
                    .ok()
                    .and_then(|t| wifi_signal(&t)),
            ),
        ]);
        for (id, value) in readings.chain(isp()) {
            if let Some(payload) = self.reporter.offer(id, value, uptime) {
                let _ = publisher.state(id, payload);
            }
        }
        self.counts = [0; 2];
        self.bytes = [0; 2];
        self.since = now;
    }
}
/// Whether the system reports available memory (the sensor is offered only then).
pub(super) fn memory_available() -> bool {
    Sampler::new(Proc::default())
        .sample()
        .memory_available
        .is_some()
}
/// The `wlan0` level field: dBm on this driver.
pub(super) fn wifi_signal(text: &str) -> Option<f64> {
    let line = text
        .lines()
        .find(|line| line.trim_start().starts_with("wlan0:"))?;
    let value = line
        .split_whitespace()
        .nth(3)?
        .trim_end_matches('.')
        .parse::<f64>()
        .ok()?;
    (value.is_finite() && (-127.0..0.0).contains(&value)).then_some(value)
}
