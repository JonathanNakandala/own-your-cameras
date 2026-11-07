//! `cam serve`: the production runtime (`--attempt TOKEN`, from the launcher) and the diagnostic pipeline.

use std::process::ExitCode;
use std::sync::atomic::AtomicBool;

use super::{CliError, OnHdal, Ownership, on_hdal};
use crate::ownership::Resource;
use crate::runtime::valid_attempt;

pub const USAGE: &str = "usage: cam serve --attempt TOKEN | cam serve --diagnostic [--port N] [--auth FILE | --no-auth] [--seconds N] [--config FILE]";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub port: u16,
    pub auth: Option<String>,
    pub seconds: u64,
    pub config_path: String,
    pub config_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Serve {
    /// Cutover runtime: lock, S00 handoff, watchdog, network and real progress.
    Production { attempt: String },
    /// RAM-only run next to a separately owned `cam wdt`: no handoff, network or health file.
    Diagnostic(Diagnostic),
}

pub fn parse(args: &[String]) -> Result<Serve, CliError> {
    let usage = || CliError::Usage;
    let mut it = args.iter();
    match it.next().map(String::as_str) {
        Some("--attempt") => {
            let token = it.next().ok_or_else(usage)?;
            if !valid_attempt(token) || it.next().is_some() {
                return Err(usage());
            }
            Ok(Serve::Production {
                attempt: token.clone(),
            })
        }
        Some("--diagnostic") => {
            let mut d = Diagnostic {
                port: 554,
                auth: Some("/mnt/para/cam-rtsp.auth".into()),
                seconds: 0,
                config_path: "/mnt/para/cam.toml".into(),
                config_required: false,
            };
            while let Some(a) = it.next() {
                if a == "--no-auth" {
                    d.auth = None;
                    continue;
                }
                let v = it.next().ok_or_else(usage)?;
                match a.as_str() {
                    "--port" => d.port = v.parse().map_err(|_| usage())?,
                    "--seconds" => d.seconds = v.parse().map_err(|_| usage())?,
                    "--auth" => d.auth = Some(v.clone()),
                    "--config" => {
                        d.config_path = v.clone();
                        d.config_required = true;
                    }
                    _ => return Err(usage()),
                }
            }
            Ok(Serve::Diagnostic(d))
        }
        _ => Err(usage()),
    }
}

pub fn owns(args: &[String]) -> Ownership {
    match parse(args) {
        Ok(Serve::Production { .. }) => Ownership::Production,
        Ok(Serve::Diagnostic(_)) => Ownership::Diagnostic(Some(Resource::Media)),
        Err(_) => Ownership::Nothing,
    }
}

pub fn run(args: &[String]) -> Result<ExitCode, CliError> {
    match parse(args)? {
        Serve::Production { attempt } => on_hdal(Production { attempt }),
        Serve::Diagnostic(d) => on_hdal(d),
    }
}

/// RAM-only, beside a separately owned `cam wdt` (two feeders must never run): no S00 handoff, network
/// supervisor or health file, and MQTT only if configured. Runs until SIGTERM or `--seconds`.
impl OnHdal for Diagnostic {
    fn run<B>(self, hdal: B) -> Result<ExitCode, CliError>
    where
        B: hdal::Backend + hdal::enc::EncBackend + hdal::AudioCap + hdal::AudioOut,
    {
        if let Err(error) = crate::signals::install() {
            eprintln!("signal setup failed: {error}");
            return Ok(ExitCode::FAILURE);
        }
        match diagnostic_with(
            &self,
            hdal,
            &crate::signals::STOP,
            crate::mqtt::device::opener(),
        ) {
            Ok(r) => {
                println!(
                    "audio: mic_frames={} mic_samples={} speaker_samples={} speaker_dropped_samples={}",
                    r.mic_frames, r.mic_samples, r.speaker_samples, r.speaker_dropped_samples
                );
                Ok(ExitCode::SUCCESS)
            }
            Err(error) => {
                eprintln!("serve: {error}");
                Ok(ExitCode::FAILURE)
            }
        }
    }
}

/// Settings, the learned focus map and the saved presets of a diagnostic run live in RAM.
pub const DIAGNOSTIC_SETTINGS: &str = "/mnt/tmp/cam-settings.toml";
pub const DIAGNOSTIC_FOCUS_MAP: &str = "/mnt/tmp/cam-focus-map.toml";
pub const DIAGNOSTIC_PRESETS: &str = "/mnt/tmp/cam-presets.toml";

/// Actual diagnostic orchestration, injected only at the camera hardware boundary.
/// Caller retains diagnostic ownership until this returns; all workers join before HDAL teardown.
pub fn diagnostic_with<
    B: hdal::Backend + hdal::enc::EncBackend + hdal::AudioCap + hdal::AudioOut,
>(
    d: &Diagnostic,
    mut backend: B,
    stop: &'static AtomicBool,
    open_hardware: crate::runtime::production::HardwareOpener,
) -> std::io::Result<crate::pipeline::AudioReport> {
    use std::sync::atomic::Ordering;
    use std::time::Instant;

    use crate::pipeline::{self, PipelineConfig};
    use crate::rtsp::audio::AudioHub;
    use crate::rtsp::hub::VideoHubs;
    use crate::rtsp::proto::Credentials;
    use crate::rtsp::server::{self, Config};
    use std::sync::Arc;

    let (port, auth, seconds) = (d.port, d.auth.clone(), d.seconds);
    let (config_path, config_required) = (d.config_path.clone(), d.config_required);
    let mut mqtt_config = match crate::mqtt::Config::read(std::path::Path::new(&config_path)) {
        Ok(cfg) => Some(cfg),
        Err(e) if !config_required && e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            return Err(std::io::Error::other(format!("{config_path}: {e}")));
        }
    }
    .filter(|cfg| cfg.mqtt.is_some());
    let workers = std::cell::RefCell::new(crate::runtime::workers::Workers::new(stop));
    let service_errors = std::cell::RefCell::new(Vec::new());
    let fail = |error: std::io::Error| {
        stop.store(true, Ordering::Relaxed);
        service_errors.borrow_mut().push(error.to_string());
    };
    let health = Arc::new(crate::mqtt::PipelineHealth::default());
    let creds = match &auth {
        None => None,
        Some(path) => match crate::files::read_private(std::path::Path::new(path), 16 * 1024)
            .ok()
            .and_then(|t| Credentials::parse(&t))
        {
            Some(c) => Some(c),
            None => {
                return Err(std::io::Error::other(format!(
                    "no usable private user:password file at {path} (use --no-auth to serve open)"
                )));
            }
        },
    };
    let listener = server::bind(port)?;
    let hubs = VideoHubs::new();
    let audio = AudioHub::new();
    let pcmu = AudioHub::with_format(camera_transport::microphone::Format::Pcmu8k);
    let talk = crate::rtsp::opus::talkback(hdal::audio::RATE);
    let mut microphone = crate::rtsp::opus::fanout(Arc::clone(&audio), Arc::clone(&pcmu))?;
    let onvif_creds = creds.clone();
    let cfg = Arc::new(Config {
        streams: hubs.paths(Some(audio), Some(pcmu)),
        creds,
        talk: Some(Arc::clone(&talk)),
    });
    let mut server_args = Some((listener, cfg));

    let mut cpu = crate::cpu::Sampler::system();
    let start = Instant::now();
    let mut open_hardware = Some(open_hardware);
    // RAM-only settings: a diagnostic run never writes /mnt/para.
    let stock_isp = crate::files::read_device_or_file(
        std::path::Path::new(crate::runtime::production::STOCK_ISP),
        16 * 1024,
    )
    .ok()
    .and_then(|b| String::from_utf8(b).ok())
    .unwrap_or_default();
    let mut owner = Some(crate::settings::Owner::load(
        Box::new(crate::settings::FileDisk::new(DIAGNOSTIC_SETTINGS)),
        &stock_isp,
        Box::new(crate::settings::FileDisk::new(DIAGNOSTIC_FOCUS_MAP)),
        mqtt_config
            .as_ref()
            .map(|c| c.presets.clone())
            .unwrap_or_default(),
        Box::new(crate::settings::FileDisk::new(DIAGNOSTIC_PRESETS)),
    ));
    let media_tuning = owner
        .as_ref()
        .map(|o| std::sync::Arc::clone(&o.media))
        .unwrap_or_default();
    // The same time keeping, motion queue and ONVIF as production (ONVIF needs credentials and a config).
    let motion = Arc::new(crate::mqtt::Motion::default());
    let _onvif = match (&mqtt_config, &onvif_creds) {
        (Some(c), Some(creds)) => {
            crate::runtime::production::start_time(c);
            crate::runtime::production::start_onvif(c, creds, &media_tuning, &motion, port)
        }
        _ => None,
    };
    let mut mqtt_motion = Some(Arc::clone(&motion));
    let counters = std::sync::Arc::new(crate::telemetry::StreamCounters::default());
    let mut telemetry = Some(crate::runtime::production::telemetry_task(
        std::sync::Arc::clone(&counters),
        std::sync::Arc::clone(&media_tuning),
    ));
    let res = pipeline::stream_with_audio(
        &mut backend,
        &PipelineConfig::e1_zoom(),
        &mut || {
            if let Some((listener, cfg)) = server_args.take() {
                if let (Some(cfg), Some(open), Some(owner), Some(telemetry), Some(motion)) = (
                    mqtt_config.take(),
                    open_hardware.take(),
                    owner.take(),
                    telemetry.take(),
                    mqtt_motion.take(),
                ) {
                    let health = Arc::clone(&health);
                    if let Err(error) = workers.borrow_mut().spawn("mqtt", move || {
                        if stop.load(Ordering::Acquire) {
                            return Ok(());
                        }
                        let hw = open(owner).map_err(std::io::Error::other)?;
                        if stop.load(Ordering::Acquire) {
                            return Ok(());
                        }
                        crate::mqtt::run_full(
                            cfg,
                            hw,
                            health,
                            stop,
                            None,
                            Some(telemetry),
                            Some(motion),
                        )
                    }) {
                        fail(error);
                        return;
                    }
                }
                println!("serve: rtsp on :{port}, audio=Opus");
                if let Err(error) = workers
                    .borrow_mut()
                    .spawn("rtsp", move || server::serve(&listener, cfg, stop))
                {
                    fail(error);
                }
            }
        },
        &mut || {
            if let Err(error) = workers.borrow_mut().shutdown() {
                fail(error);
            }
        },
        &mut || {
            if let Err(error) = workers.borrow_mut().check() {
                fail(error);
            }
            !stop.load(Ordering::Relaxed) && (seconds == 0 || start.elapsed().as_secs() < seconds)
        },
        &mut |f| {
            health.frame(f.stream);
            counters.frame(f.stream, f.data.len());
            hubs.publish(f.stream, f.key, f.pts_us, f.data);
        },
        &mut |f| {
            microphone.push(f.pts_us, &f.samples);
        },
        &mut || talk.next_pcm(Instant::now()),
        &mut &*media_tuning,
    );
    if let Some(busy) = cpu.sample_system() {
        println!("cpu_busy_percent={busy:.1}");
    }
    let mut errors = service_errors.into_inner();
    match res {
        Ok(report) if errors.is_empty() => Ok(report),
        result => {
            if let Err(error) = result {
                errors.push(format!("media: {error}"));
            }
            Err(std::io::Error::other(errors.join("; ")))
        }
    }
}

struct Production {
    attempt: String,
}

/// Preflight, runtime lock, process exclusion and the RTSP bind happen before the watchdog handoff
/// (`production::start`); then one lifecycle owner runs the feeder, network, media, RTSP and MQTT. It never
/// disarms: every exit leaves the armed watchdog to reset the boot, with the network children still running.
impl OnHdal for Production {
    fn run<B>(self, hdal: B) -> Result<ExitCode, CliError>
    where
        B: hdal::Backend + hdal::enc::EncBackend + hdal::AudioCap + hdal::AudioOut,
    {
        use crate::runtime::production::{self, Paths};
        use crate::signals;

        let attempt = &self.attempt;
        let clock = crate::runtime::Monotonic::default();
        if let Err(error) = signals::install() {
            eprintln!("signal setup failed: {error}");
            return Ok(ExitCode::FAILURE);
        }
        let started = match production::start(
            &Paths::system(),
            attempt,
            &signals::STOP,
            hdal,
            clock,
            crate::mqtt::device::opener(),
        ) {
            Ok(started) => started,
            Err(e) => {
                eprintln!("serve: {e}");
                return Ok(ExitCode::FAILURE);
            }
        };
        // Bounded logs before any service; failing to bound them refuses the start.
        if let Err(e) = crate::logcap::redirect(
            std::path::Path::new(crate::logcap::LOG_FILE),
            crate::logcap::CAP_BYTES,
            &[1, 2],
        ) {
            eprintln!("serve: cannot open {}: {e}", crate::logcap::LOG_FILE);
            return Ok(ExitCode::FAILURE);
        }
        println!("serve: production attempt {attempt} starting");
        let network = started.network;
        // ONVIF serves while production runs and stops (sending Bye) when it returns.
        let _onvif = started.onvif;
        let outcome = production::run(started.wiring);
        let code = match &outcome.result {
            Ok(report) => {
                println!(
                    "serve: stopped; mic_frames={} speaker_samples={}",
                    report.mic_frames, report.speaker_samples
                );
                0
            }
            Err(e) => {
                eprintln!("serve: failed: {e}");
                1
            }
        };
        // The runtime lock (in `outcome`) and the network children stay to the end of the process; with the
        // feeder gone, the armed watchdog resets the boot.
        let host = network.lock().ok().and_then(|mut slot| slot.take());
        match host {
            Some(host) => host.exit_preserving_network(code),
            None => std::process::exit(code),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn production_takes_only_a_valid_attempt_token() {
        assert_eq!(
            parse(&args(&["--attempt", "boot-1_a2"])),
            Ok(Serve::Production {
                attempt: "boot-1_a2".into()
            })
        );
        for bad in [
            args(&[]),
            args(&["--attempt"]),
            args(&["--attempt", ""]),
            args(&["--attempt", "../x"]),
            args(&["--attempt", &"a".repeat(65)]),
            args(&["--attempt", "ok", "--no-auth"]),
            args(&["--attempt", "ok", "--seconds", "5"]),
            args(&["--port", "554"]),
            args(&["--auth", "/mnt/tmp/x"]),
            args(&["--seconds", "120"]),
        ] {
            assert!(parse(&bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn diagnostic_keeps_the_old_flags() {
        let Ok(Serve::Diagnostic(d)) = parse(&args(&[
            "--diagnostic",
            "--auth",
            "/mnt/tmp/cam-rtsp.auth",
            "--seconds",
            "300",
            "--port",
            "8554",
            "--config",
            "/mnt/tmp/c.toml",
        ])) else {
            panic!("diagnostic must parse");
        };
        assert_eq!(d.auth.as_deref(), Some("/mnt/tmp/cam-rtsp.auth"));
        assert_eq!((d.seconds, d.port), (300, 8554));
        assert!(d.config_required);
        let Ok(Serve::Diagnostic(open)) = parse(&args(&["--diagnostic", "--no-auth"])) else {
            panic!();
        };
        assert_eq!(open.auth, None);
        assert!(!open.config_required);
    }

    #[test]
    fn diagnostic_rejects_bad_values_and_the_production_token() {
        for bad in [
            args(&["--diagnostic", "--port", "99999"]),
            args(&["--diagnostic", "--seconds", "x"]),
            args(&["--diagnostic", "--bogus"]),
            args(&["--diagnostic", "--auth"]),
            args(&["--diagnostic", "--attempt", "t"]),
        ] {
            assert!(parse(&bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn bare_old_style_serve_is_refused() {
        assert!(
            parse(&args(&[
                "--auth",
                "/mnt/tmp/cam-rtsp.auth",
                "--seconds",
                "120"
            ]))
            .is_err()
        );
    }
}
