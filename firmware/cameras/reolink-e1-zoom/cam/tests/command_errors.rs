//! Command errors at OS signal/output/watchdog boundaries.
#[test]
fn signal_installation_reports_an_uncatchable_signal() {
    assert!(cam::signals::install_handler(libc::SIGKILL).is_err());
}

#[test]
fn health_watch_returns_failure_when_output_write_or_flush_fails() {
    use std::io::{self, Write};
    struct ClosedOutput(bool, Vec<u8>);
    impl Write for ClosedOutput {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.0 {
                self.1.extend_from_slice(bytes);
                Ok(bytes.len())
            } else {
                Err(io::ErrorKind::BrokenPipe.into())
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
    }
    let args = cam::cmd::health::Args {
        attempt: "output".into(),
        pid: 1,
        dir: "/mnt/tmp".into(),
    };
    for flush in [false, true] {
        let mut output = ClosedOutput(flush, Vec::new());
        assert_eq!(
            cam::cmd::health::watch_to(&args, &mut output),
            std::process::ExitCode::FAILURE
        );
        assert_eq!(
            output.1,
            if flush {
                b"unqualified\n".as_slice()
            } else {
                b"".as_slice()
            },
            "no second transition after output fails"
        );
    }
}

#[test]
fn diagnostic_watchdog_errors_are_terminal_and_do_not_disarm_after_a_feed_failure() {
    use std::cell::Cell;
    use std::io;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    struct Clock<'a>(Duration, &'a AtomicBool);
    impl cam::runtime::Clock for Clock<'_> {
        fn now(&self) -> Duration {
            self.0
        }
        fn wait(&mut self, d: Duration) {
            self.0 += d;
            if self.0 >= Duration::from_secs(11) {
                self.1.store(true, Ordering::Relaxed);
            }
        }
    }
    struct Device<'a> {
        fail_feed: bool,
        feeds: u32,
        disarmed: &'a Cell<bool>,
    }
    impl cam::wdt::WatchdogDevice for Device<'_> {
        fn set_timeout(&mut self, secs: u32) -> io::Result<u32> {
            Ok(secs)
        }
        fn keepalive(&mut self) -> io::Result<()> {
            self.feeds += 1;
            if self.fail_feed && self.feeds > 1 {
                Err(io::Error::other("feed fault"))
            } else {
                Ok(())
            }
        }
        fn disarm(&mut self) -> io::Result<()> {
            self.disarmed.set(true);
            Err(io::Error::other("disarm fault"))
        }
    }
    for fail_feed in [true, false] {
        let stop = AtomicBool::new(false);
        let disarmed = Cell::new(false);
        let mut clock = Clock(Duration::ZERO, &stop);
        let result = cam::cmd::wdt::run_with(
            || {
                Ok(Device {
                    fail_feed,
                    feeds: 0,
                    disarmed: &disarmed,
                })
            },
            &mut clock,
            &stop,
        );
        assert!(result.unwrap_err().to_string().contains(if fail_feed {
            "feed fault"
        } else {
            "disarm fault"
        }));
        assert_eq!(disarmed.get(), !fail_feed);
    }
    let stop = AtomicBool::new(false);
    let mut clock = Clock(Duration::ZERO, &stop);
    let result = cam::cmd::wdt::run_with::<Device<'_>, _>(
        || Err(io::Error::from_raw_os_error(libc::EBUSY)),
        &mut clock,
        &stop,
    );
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
    assert_eq!(clock.0, Duration::from_secs(10));
}

#[test]
fn argumentless_commands_reject_options_before_acquiring_ownership() {
    for command in ["wdt", "probe-videocap", "self-test"] {
        let args = vec![command.into(), "--unexpected".into()];
        let result =
            cam::cmd::dispatch(&args, std::path::Path::new("/nonexistent/cam-runtime.lock"));
        assert_eq!(
            result.unwrap(),
            std::process::ExitCode::from(2),
            "{command}"
        );
    }
}

#[test]
fn stop_signal_process_probe() {
    if std::env::var_os("CAM_TEST_STOP_SIGNAL").is_none() {
        return;
    }
    cam::signals::install().unwrap();
    assert!(
        std::process::Command::new("kill")
            .args(["-TERM", &std::process::id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    assert!(!cam::signals::nap(std::time::Duration::from_secs(1)));
}

#[test]
fn installed_handler_delivers_sigterm_to_the_command_stop_flag() {
    assert!(
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "stop_signal_process_probe"])
            .env("CAM_TEST_STOP_SIGNAL", "1")
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn probe_command_fails_when_only_cleanup_failed() {
    let mut hw = hdal::fake::Fake::failing(hdal::Step::VideocapStop, -8);
    assert_eq!(
        cam::cmd::dump::probe_with(&mut hw),
        Ok(std::process::ExitCode::FAILURE)
    );
    assert_eq!(hw.calls.last(), Some(&"common_uninit"));
    assert_eq!(
        cam::cmd::dump::probe_with(&mut hdal::fake::Fake::default()),
        Ok(std::process::ExitCode::SUCCESS)
    );
}

/// Usage errors the binary reports before taking any ownership lease: exit code and first stderr line.
#[test]
fn usage_matrix_before_any_lease() {
    let serve = "usage: cam serve --attempt TOKEN | cam serve --diagnostic [--port N] [--auth FILE | --no-auth] [--seconds N] [--config FILE] [--retune SECONDS:WxH@FPS@KBPS[@gdr]]...";
    let top = "usage: cam self-test | probe-videocap | dump | serve | preflight | health-watch | ptz ... | zoom ... | focus ... | ir ... | led ... | wdt | net";
    for (args, stderr) in [
        (&[][..], top),
        (&["bogus"][..], top),
        (&["wdt", "x"][..], "usage: cam wdt"),
        (&["probe-videocap", "x"][..], "usage: cam probe-videocap"),
        (&["self-test", "x"][..], "usage: cam self-test"),
        (&["serve"][..], serve),
        (&["serve", "--attempt", "../x"][..], serve),
        (&["serve", "--diagnostic", "--port", "99999"][..], serve),
        (&["preflight", "x"][..], "usage: cam preflight"),
        (
            &["health-watch"][..],
            "usage: cam health-watch --attempt TOKEN --pid PID [--dir DIR]",
        ),
        (
            &["net", "--bogus"][..],
            "usage: cam net [--check | --seed-vendor]",
        ),
        (
            &["net", "--check", "x"][..],
            "usage: cam net [--check | --seed-vendor]",
        ),
    ] {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_cam"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert_eq!(
            String::from_utf8_lossy(&out.stderr).lines().next(),
            Some(stderr),
            "{args:?}"
        );
        assert!(out.stdout.is_empty(), "{args:?}");
    }
}

#[test]
fn the_hardware_owners_are_exactly_the_commands_that_take_a_lease() {
    let owners: Vec<_> = cam::cmd::COMMANDS
        .iter()
        .map(|c| c.name)
        .filter(|name| cam::cmd::owns_hardware(name))
        .collect();
    assert_eq!(
        owners,
        [
            "probe-videocap",
            "dump",
            "serve",
            "ptz",
            "zoom",
            "focus",
            "ir",
            "led",
            "wdt",
            "net"
        ]
    );
    assert!(!cam::cmd::owns_hardware("cam"));
}

/// Bad words exit 2 with the usage after the leases are taken; bad values are failures (exit 1). The device is
/// never opened in either case.
#[test]
fn leased_commands_check_their_arguments_before_opening_hardware() {
    let root = std::env::temp_dir().join(format!("cam-usage-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let runtime = root.join("cam-runtime.lock");
    for (args, code) in [
        (&["ptz", "fly"][..], 2),
        (&["ptz", "move", "a", "b"][..], 1),
        (&["ptz", "--dev"][..], 1),
        (&["zoom"][..], 2),
        (&["zoom", "far"][..], 1),
        (&["focus", "1", "2"][..], 2),
        (&["ir", "dusk"][..], 2),
        (&["ir", "auto", "--to-night"][..], 2),
        (&["ir", "auto", "--to-night", "x"][..], 1),
        (&["led"][..], 2),
        (&["dump", "--seconds", "x"][..], 2),
    ] {
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        assert_eq!(
            cam::cmd::dispatch(&args, &runtime).unwrap(),
            std::process::ExitCode::from(code),
            "{args:?}"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}
