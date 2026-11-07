use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use cam::identity::Identity;
use cam::net::linux::LinuxHost;

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "cam-network-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn write(&self, path: &str, data: impl AsRef<[u8]>) -> PathBuf {
        let path = self.0.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, data).unwrap();
        if path.ends_with("cam-wifi.conf") {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn production_preflight_requires_persistent_usable_credentials_without_vendor_config() {
    let f = Fixture::new();
    let config = f.0.join("mnt/para/cam-wifi.conf");
    assert!(LinuxHost::production_at(&f.0, &Identity::default()).is_err());
    let valid = "network={\n ssid=\"fixture-ap\"\n psk=\"fixture-password\"\n}\n";
    f.write("mnt/para/cam-wifi.conf", valid);
    assert!(LinuxHost::production_at(&f.0, &Identity::default()).is_ok());
    assert!(!f.0.join("mnt/tmp/atheros_config.conf").exists());
    for invalid in [
        "",
        "network={\nssid=\"fixture-ap\"\n}\n",
        "network={\nssid=\"fixture-ap\"\npsk=\"short\"\n}\n",
        "network={\nssid=\"fixture-ap\"\npsk=\"fixture-password\"\n",
        "network={\nssid=\"\"\npsk=\"fixture-password\"\n}\n",
        "network={\nssid=\"fixture-ap\"\npsk=\"fixture-password\"\ndisabled=1\n}\n",
    ] {
        fs::write(&config, invalid).unwrap();
        let error = LinuxHost::production_at(&f.0, &Identity::default())
            .err()
            .expect("must reject invalid credentials")
            .to_string();
        assert!(error.contains("cam-wifi.conf"), "{error}");
        assert!(!error.contains("fixture-password"));
    }
    fs::write(&config, valid).unwrap();
    assert_eq!(fs::read_to_string(&config).unwrap(), valid);
    assert!(!Path::new(&f.0.join("mnt/para/cam.enabled")).exists());
}

use cam::net::{Host, Proc};
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};
impl Fixture {
    fn host(&self) -> LinuxHost {
        self.write(
            "mnt/para/cam-wifi.conf",
            "network={\nssid=\"fixture-ap\"\npsk=\"fixture-password\"\n}\n",
        );
        fs::create_dir_all(self.0.join("proc")).unwrap();
        self.write("proc/modules", "");
        LinuxHost::production_at(&self.0, &Identity::default()).unwrap()
    }
    /// Writes an executable through a separate `install` process: a file this process writes and then
    /// executes can fail with ETXTBSY on Linux when another test thread forks while it is still open.
    fn script(&self, name: &str, body: &str) {
        let source = self.write(
            &format!("script-src/{name}"),
            format!("#!/bin/sh\ncd '{}'\n{body}\n", self.0.display()),
        );
        let target = self.0.join("sbin").join(name);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        let status = std::process::Command::new("install")
            .args(["-m", "700"])
            .arg(&source)
            .arg(&target)
            .status()
            .unwrap();
        assert!(status.success(), "install {name}");
    }
}
fn until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready() {
        assert!(Instant::now() < deadline, "process fixture timed out");
        std::thread::yield_now();
    }
}
#[test]
fn noisy_running_children_and_respawns_have_zero_log_budget() {
    let f = Fixture::new();
    let mut host = f.host();
    f.script("udhcpc", "dd if=/dev/zero bs=65536 count=8\ndd if=/dev/zero bs=65536 count=8 >&2\ntouch emitted\nexec sleep 30");
    for _ in 0..2 {
        host.spawn(Proc::DhcpWlan).unwrap();
        until(|| f.0.join("emitted").exists());
        let alive = host.alive(Proc::DhcpWlan).unwrap();
        let bytes: u64 = fs::read_dir(f.0.join("mnt/tmp"))
            .into_iter()
            .flatten()
            .map(|e| e.unwrap().metadata().unwrap().len())
            .sum();
        host.kill(Proc::DhcpWlan);
        fs::remove_file(f.0.join("emitted")).unwrap();
        assert!(alive, "budget must hold during the child's lifetime");
        assert_eq!(
            bytes, 0,
            "network children must not accumulate tmpfs output"
        );
    }
}

#[test]
fn failed_rmmod_still_removes_second_module_and_powers_off() {
    let f = Fixture::new();
    let mut host = f.host();
    f.write("sys/class/gpio/gpio133/direction", "out");
    let power = f.write("sys/class/gpio/gpio133/value", "1");
    f.script("rmmod", "echo \"$1\" >> removals\nexit 1");
    let error = host.unload_wifi_modules().unwrap_err().to_string();
    assert_eq!(
        fs::read_to_string(f.0.join("removals")).unwrap(),
        "wlan\ncfg80211\n"
    );
    assert_eq!(fs::read_to_string(power).unwrap(), "0");
    assert!(
        error.contains("wlan") && error.contains("cfg80211"),
        "{error}"
    );
}

#[test]
fn hung_helpers_are_bounded_and_their_process_groups_are_stopped() {
    let f = Fixture::new();
    let mut host = f.host();
    f.write("sys/class/gpio/gpio133/direction", "out");
    let power = f.write("sys/class/gpio/gpio133/value", "1");
    f.script(
        "rmmod",
        "echo $$ >> helper-pids\nsleep 3 &\necho $! >> helper-pids\nwait",
    );
    let start = Instant::now();
    let result = host.unload_wifi_modules();
    let elapsed = start.elapsed();
    assert!(result.is_err(), "a helper exceeding its deadline must fail");
    assert!(
        elapsed < Duration::from_millis(5500),
        "cleanup took {elapsed:?}"
    );
    assert_eq!(fs::read_to_string(power).unwrap(), "0");
    let pids = fs::read_to_string(f.0.join("helper-pids")).unwrap();
    assert_eq!(pids.lines().count(), 4, "both removals must run");
    for pid in pids.lines() {
        until(|| {
            !std::process::Command::new("/bin/kill")
                .args(["-0", pid])
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap()
                .success()
        });
    }
}

#[test]
fn failed_loads_reach_power_off_holdoff_and_retry_without_blocking_ethernet() {
    use cam::net::{Event, Supervisor, Timing};
    let f = Fixture::new();
    let mut host = f.host();
    f.write("sys/class/gpio/gpio133/direction", "out");
    let power = f.write("sys/class/gpio/gpio133/value", "0");
    f.script("insmod", "echo load >> loads\nexit 1");
    f.script("rmmod", "echo \"$1\" >> removals\nexit 1");
    f.script("modprobe", "echo eth >> ethernet");
    let mut supervisor = Supervisor::new(Timing {
        restart_delay: Duration::ZERO,
        ..Timing::default()
    });
    let first = supervisor.tick(&mut host, Duration::ZERO);
    assert!(
        first
            .iter()
            .any(|e| matches!(e, Event::Failed("load wifi", _)))
    );
    assert!(first.contains(&Event::EthModuleLoaded));
    supervisor.tick(&mut host, Duration::from_secs(10));
    let off_at = Instant::now();
    assert_eq!(
        fs::read_to_string(&power).unwrap(),
        "0",
        "failed insmod must still reach power-off"
    );
    assert_eq!(
        fs::read_to_string(f.0.join("removals")).unwrap(),
        "wlan\ncfg80211\n"
    );
    // Even a stale sysfs interface after failed rmmod must not bypass the powered-off interval.
    f.write("sys/class/net/wlan0/flags", "0x1");
    supervisor.tick(&mut host, Duration::from_millis(10499));
    assert_eq!(fs::read_to_string(&power).unwrap(), "0");
    assert_eq!(fs::read_to_string(f.0.join("loads")).unwrap(), "load\n");
    until(|| off_at.elapsed() >= Duration::from_millis(500));
    supervisor.tick(&mut host, Duration::from_millis(10500));
    assert_eq!(fs::read_to_string(&power).unwrap(), "1");
    assert_eq!(
        fs::read_to_string(f.0.join("loads")).unwrap(),
        "load\nload\n"
    );
    assert_eq!(
        fs::read_to_string(f.0.join("ethernet"))
            .unwrap()
            .lines()
            .count(),
        4
    );
}

fn running(pid: &str) -> bool {
    std::process::Command::new("/bin/kill")
        .args(["-0", pid])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success()
}
#[test]
fn duplicate_spawn_does_not_lose_ownership_of_the_first_child() {
    let f = Fixture::new();
    let mut host = f.host();
    f.script("udhcpc", "echo $$ > first-pid\nexec sleep 30");
    host.spawn(Proc::DhcpWlan).unwrap();
    until(|| f.0.join("first-pid").exists());
    let pid = fs::read_to_string(f.0.join("first-pid")).unwrap();
    host.spawn(Proc::DhcpWlan).unwrap();
    host.kill(Proc::DhcpWlan);
    let survived = running(pid.trim());
    if survived {
        std::process::Command::new("/bin/kill")
            .args(["-KILL", pid.trim()])
            .status()
            .unwrap();
    }
    assert!(
        !survived,
        "a repeated spawn must not overwrite the owned child handle"
    );
}

#[test]
fn dropping_the_host_stops_and_reaps_owned_children_but_keeps_credentials() {
    let f = Fixture::new();
    let mut host = f.host();
    f.script("udhcpc", "echo $$ > owned-pid\nexec sleep 30");
    host.spawn(Proc::DhcpWlan).unwrap();
    until(|| f.0.join("owned-pid").exists());
    let pid = fs::read_to_string(f.0.join("owned-pid")).unwrap();
    drop(host);
    let survived = running(pid.trim());
    if survived {
        std::process::Command::new("/bin/kill")
            .args(["-KILL", pid.trim()])
            .status()
            .unwrap();
    }
    assert!(!survived, "owned child must be reaped by shutdown/drop");
    assert!(
        LinuxHost::production_at(&f.0, &Identity::default()).is_ok(),
        "credentials survive shutdown"
    );
}

#[test]
fn network_exit_fixture() {
    let Some(root) = std::env::var_os("CAM_TEST_EXIT_ROOT") else {
        return;
    };
    let mut host = LinuxHost::production_at(Path::new(&root), &Identity::default()).unwrap();
    host.spawn(Proc::DhcpWlan).unwrap();
    until(|| Path::new(&root).join("survivor-pid").exists());
    host.exit_preserving_network(0);
}

#[test]
fn deliberate_exit_preserves_children_without_log_threads_or_a_live_parent() {
    let f = Fixture::new();
    drop(f.host());
    f.script("udhcpc", "echo $$ > survivor-pid\nexec sleep 30");
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "network_exit_fixture"])
        .env("CAM_TEST_EXIT_ROOT", &f.0)
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    let pid = fs::read_to_string(f.0.join("survivor-pid")).unwrap();
    let survived = running(pid.trim());
    if survived {
        std::process::Command::new("/bin/kill")
            .args(["-KILL", pid.trim()])
            .status()
            .unwrap();
        until(|| !running(pid.trim()));
    }
    assert!(status.success());
    assert!(
        survived,
        "intentional process exit must leave networking reachable until reboot"
    );
    assert!(LinuxHost::production_at(&f.0, &Identity::default()).is_ok());
}

struct ExternalChild(std::process::Child);
impl Drop for ExternalChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[test]
fn adopted_interface_processes_are_not_duplicated_or_killed() {
    let f = Fixture::new();
    let mut vendor = ExternalChild(
        std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap(),
    );
    let proc_path = format!("proc/{}/cmdline", vendor.0.id());
    // /proc records are isolated fixtures; lifetime/ownership uses a real external child.
    for (kind, cmdline) in [
        (Proc::Wpa, b"/sbin/wpa_supplicant\0-i\0wlan0\0".as_slice()),
        (Proc::Wpa, b"wpa_supplicant\0-iwlan0\0".as_slice()),
        (Proc::DhcpWlan, b"udhcpc\0-iwlan0\0".as_slice()),
        (Proc::DhcpEth, b"udhcpc\0-i\0eth0\0".as_slice()),
    ] {
        let mut host = f.host();
        f.write(&proc_path, cmdline);
        assert!(
            host.alive(kind).unwrap(),
            "must recognize {kind:?} by its interface option"
        );
        host.spawn(kind).unwrap(); // no executable in fixture: any duplicate attempt fails
        host.kill(kind);
        host.shutdown();
        drop(host);
        assert!(vendor.0.try_wait().unwrap().is_none());
    }
    f.write(&proc_path, b"udhcpc\0-i\0eth1\0-x\0wlan0\0");
    assert!(
        !f.host().alive(Proc::DhcpWlan).unwrap(),
        "unrelated argument isn't an interface option"
    );
}

#[test]
fn power_off_interval_starts_after_slow_removal_finishes() {
    let f = Fixture::new();
    let mut host = f.host();
    f.write("sys/class/gpio/gpio133/direction", "out");
    let power = f.write("sys/class/gpio/gpio133/value", "1");
    f.script("rmmod", "sleep 1");
    host.unload_wifi_modules().unwrap();
    assert_eq!(fs::read_to_string(&power).unwrap(), "0");
    let off = Instant::now();
    let early = host.power_wifi();
    assert!(
        early.is_err(),
        "slow rmmod must not consume the powered-off interval"
    );
    until(|| host.power_wifi().is_ok());
    assert!(off.elapsed() >= Duration::from_millis(490));
    assert_eq!(fs::read_to_string(&power).unwrap(), "1");
}

#[test]
fn diagnostic_seeding_validates_before_writing_and_never_replaces_persistent_credentials() {
    use cam::net::linux::seed_wpa_conf;
    let f = Fixture::new();
    let ours = f.0.join("wifi.conf");
    let vendor = f.write(
        "vendor.conf",
        "network={\nssid=\"fixture-ap\"\npsk=\"short\"\n}\n",
    );
    assert!(seed_wpa_conf(&ours, &vendor).is_err());
    assert!(
        !ours.exists(),
        "invalid credentials must not become persistent"
    );
    let valid = "network={\nssid=\"fixture-ap\"\npsk=\"fixture-password\"\n}\n";
    fs::write(&vendor, valid).unwrap();
    seed_wpa_conf(&ours, &vendor).unwrap();
    assert_eq!(
        fs::metadata(&ours).unwrap().permissions().mode() & 0o777,
        0o600
    );
    fs::remove_file(&vendor).unwrap();
    seed_wpa_conf(&ours, &vendor).unwrap();
    assert_eq!(fs::read_to_string(&ours).unwrap(), valid);
    fs::write(&ours, "broken").unwrap();
    assert!(seed_wpa_conf(&ours, &vendor).is_err());
    assert_eq!(
        fs::read_to_string(&ours).unwrap(),
        "broken",
        "never silently overwrite existing config"
    );
}

#[test]
fn network_cli_rejects_unknown_options_before_attempting_startup() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_cam"))
        .args(["net", "--unknown"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("--check") && error.contains("--seed-vendor"));
}

#[test]
fn completed_children_do_not_leave_helper_descendants_behind() {
    let f = Fixture::new();
    let mut host = f.host();
    f.script("udhcpc", "sleep 30 &\necho $! > descendant-pid\nexit 0");
    host.spawn(Proc::DhcpWlan).unwrap();
    until(|| {
        fs::read_to_string(f.0.join("descendant-pid"))
            .is_ok_and(|s| s.trim().parse::<u32>().is_ok())
    });
    let pid = fs::read_to_string(f.0.join("descendant-pid")).unwrap();
    until(|| !host.alive(Proc::DhcpWlan).unwrap());
    // A killed orphan may briefly await init's reap; observe execution state, not PID existence.
    let still_running = || {
        let state = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", pid.trim()])
            .output()
            .unwrap();
        let state = String::from_utf8_lossy(&state.stdout);
        !state.trim().is_empty() && !state.trim().starts_with('Z')
    };
    // SIGKILL delivery is asynchronous, even after the already-exited leader is reaped.
    // Observe descendant exit within a deadline instead of racing its next scheduling turn.
    let deadline = Instant::now() + Duration::from_secs(2);
    while still_running() && Instant::now() < deadline {
        std::thread::yield_now();
    }
    let survived = still_running();
    if survived {
        std::process::Command::new("/bin/kill")
            .args(["-KILL", pid.trim()])
            .status()
            .unwrap();
    }
    until(|| !running(pid.trim()));
    assert!(
        !survived,
        "observing child exit must also clean its helper process group"
    );
}

#[test]
fn network_preflight_refuses_a_symlink_in_the_persistent_profile_path() {
    let f = Fixture::new();
    let source = f.write(
        "mnt/para/source",
        "network={\nssid=\"fixture-ap\"\npsk=\"fixture-password\"\n}\n",
    );
    std::os::unix::fs::symlink(source, f.0.join("mnt/para/cam-wifi.conf")).unwrap();
    assert!(LinuxHost::production_at(&f.0, &Identity::default()).is_err());
}

#[test]
fn failed_wlan_up_still_starts_ethernet_after_the_grace_period() {
    use cam::net::{Event, Supervisor, Timing};
    let f = Fixture::new();
    let mut host = f.host();
    f.write("sys/class/net/wlan0/flags", "0x0");
    f.write("sys/class/net/eth0/flags", "0x1");
    f.write("sys/class/net/eth0/carrier", "1");
    f.script("ifconfig", "exit 1");
    f.script("udhcpc", "echo started > ethernet\nexec sleep 30");
    let mut supervisor = Supervisor::new(Timing::default());
    supervisor.tick(&mut host, Duration::ZERO);
    assert!(!f.0.join("ethernet").exists());
    let events = supervisor.tick(&mut host, Duration::from_secs(30));
    assert!(
        events.contains(&Event::Spawned(Proc::DhcpEth)),
        "{events:?}"
    );
    until(|| f.0.join("ethernet").exists());
}

#[test]
fn unreadable_process_inventory_never_authorizes_a_duplicate_child() {
    let f = Fixture::new();
    let mut host = f.host();
    f.script("udhcpc", "echo duplicate > duplicate\nexec sleep 30");
    // A surviving PID with an unreadable cmdline, followed by a missing proc mount.
    fs::create_dir_all(f.0.join("proc/123/cmdline")).unwrap();
    let unreadable = host.spawn(Proc::DhcpWlan);
    host.shutdown();
    fs::remove_dir_all(f.0.join("proc")).unwrap();
    let missing = host.spawn(Proc::DhcpWlan);
    host.shutdown();
    assert!(unreadable.is_err(), "unknown identity must refuse spawning");
    assert!(missing.is_err(), "missing proc mount must refuse spawning");
    assert!(!f.0.join("duplicate").exists());
}

#[test]
fn empty_surviving_userspace_identity_does_not_authorize_spawn() {
    let f = Fixture::new();
    let mut host = f.host();
    f.script("udhcpc", "exec sleep 30");
    f.write("proc/123/cmdline", b"");
    f.write("proc/123/stat", "123 (client) R 1 2 3 4 5 0");
    assert!(host.spawn(Proc::DhcpWlan).is_err());
    for stat in [
        "123 (client) Z 1 2 3 4 5 0",
        "123 (kernel) S 1 2 3 4 5 2097152",
    ] {
        f.write("proc/123/stat", stat);
        host.spawn(Proc::DhcpWlan).unwrap();
        host.shutdown();
    }
}

#[test]
fn network_inspection_fixture() {
    let Some(root) = std::env::var_os("CAM_TEST_INSPECT_ROOT") else {
        return;
    };
    let mut host = LinuxHost::production_at(Path::new(&root), &Identity::default()).unwrap();
    assert!(host.spawn(Proc::DhcpWlan).is_err());
}

#[test]
fn process_inspection_rejects_oversized_and_blocking_records_promptly() {
    let f = Fixture::new();
    drop(f.host());
    f.script("udhcpc", "exec sleep 30");
    let path = f.write("proc/123/cmdline", vec![b'x'; 65537]);
    let mut failures = Vec::new();
    for kind in ["oversized", "fifo"] {
        if kind == "fifo" {
            fs::remove_file(&path).unwrap();
            assert!(
                std::process::Command::new("mkfifo")
                    .arg(&path)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let mut child = ExternalChild(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "network_inspection_fixture"])
                .env("CAM_TEST_INSPECT_ROOT", &f.0)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        let status = loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                break Some(status);
            }
            if Instant::now() >= deadline {
                break None;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        if !status.is_some_and(|s| s.success()) {
            failures.push(kind);
        }
    }
    assert!(failures.is_empty(), "did not promptly reject: {failures:?}");
}

#[test]
fn adoption_does_not_mistake_option_values_for_interface_arguments() {
    let f = Fixture::new();
    let mut host = f.host();
    for (kind, cmdline) in [
        (Proc::DhcpWlan, b"udhcpc\0-ieth1\0-x\0-iwlan0\0".as_slice()),
        (
            Proc::Wpa,
            b"wpa_supplicant\0-ieth1\0-c\0-iwlan0\0".as_slice(),
        ),
    ] {
        f.write("proc/123/cmdline", cmdline);
        assert!(
            !host.alive(kind).unwrap(),
            "option value falsely adopted for {kind:?}"
        );
    }
    // An ambiguous same-program invocation must not authorize another client.
    f.write("proc/123/cmdline", b"udhcpc\0--unknown\0");
    assert!(host.alive(Proc::DhcpWlan).is_err());
}

#[test]
fn unknown_module_inventory_does_not_run_insmod_and_is_reported() {
    use cam::net::{Event, Supervisor, Timing};
    let f = Fixture::new();
    let mut host = f.host();
    f.script("insmod", "echo unexpected > module-loaded");
    f.script("modprobe", "echo unexpected > module-loaded");
    f.write("sys/class/gpio/gpio133/value", "0");
    f.write("sys/class/gpio/gpio133/direction", "out");
    f.write("proc/modules", vec![b'x'; 65537]);
    let result = host.load_wifi_modules();
    let events = Supervisor::new(Timing::default()).tick(&mut host, Duration::ZERO);
    assert!(result.is_err());
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::Failed("inspect eth module", _))),
        "{events:?}"
    );
    assert!(!f.0.join("module-loaded").exists());
}

#[test]
fn persistent_network_entry_points_refuse_public_or_hardlinked_credentials() {
    let f = Fixture::new();
    drop(f.host());
    let ours = f.0.join("mnt/para/cam-wifi.conf");
    let vendor = f.0.join("absent-vendor");
    fs::set_permissions(&ours, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(LinuxHost::production_at(&f.0, &Identity::default()).is_err());
    assert!(cam::net::linux::seed_wpa_conf(&ours, &vendor).is_err());
    fs::set_permissions(&ours, fs::Permissions::from_mode(0o600)).unwrap();
    fs::hard_link(&ours, f.0.join("linked-secret")).unwrap();
    assert!(LinuxHost::production_at(&f.0, &Identity::default()).is_err());
    assert!(cam::net::linux::seed_wpa_conf(&ours, &vendor).is_err());
}

#[test]
fn a_second_network_diagnostic_is_refused_before_command_startup() {
    let f = Fixture::new();
    let runtime = f.0.join("cam-runtime.lock");
    let network = f.0.join("cam-network.lock");
    let _owner =
        cam::runtime::s00::S00Handoff::acquire(&network, &f.0.join("flag"), &f.0.join("wdt"))
            .unwrap();
    let result = cam::cmd::dispatch(&["net".into()], &runtime);
    assert!(
        result.is_err(),
        "two diagnostic network supervisors may race adoption and power cycling"
    );
    assert!(!f.0.join("flag").exists());
}

#[test]
fn child_order_fixture() {
    let Some(root) = std::env::var_os("CAM_TEST_CHILD_ROOT") else {
        return;
    };
    let mut host = LinuxHost::production_at(Path::new(&root), &Identity::default()).unwrap();
    fs::write(
        Path::new(&root).join("observer-pid"),
        std::process::id().to_string(),
    )
    .unwrap();
    host.spawn(Proc::DhcpWlan).unwrap();
    until(|| !host.alive(Proc::DhcpWlan).unwrap());
}

#[test]
fn the_group_leader_is_not_reaped_until_after_its_group_is_signalled() {
    let f = Fixture::new();
    drop(f.host());
    f.script("udhcpc", "exit 0");
    let library = f.0.join("child-order.dylib");
    let trace = f.0.join("syscalls");
    let mut cc = std::process::Command::new("cc");
    cc.args(["-shared", "-fPIC", "-o"])
        .arg(&library)
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/child_order.c"));
    if cfg!(target_os = "linux") {
        cc.arg("-ldl");
    }
    assert!(cc.status().unwrap().success());
    let mut child = ExternalChild(
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "child_order_fixture"])
            .env("CAM_TEST_CHILD_ROOT", &f.0)
            .env("CAM_TEST_CHILD_TRACE", &trace)
            .env(
                if cfg!(target_os = "macos") {
                    "DYLD_INSERT_LIBRARIES"
                } else {
                    "LD_PRELOAD"
                },
                &library,
            )
            .spawn()
            .unwrap(),
    );
    until(|| child.0.try_wait().unwrap().is_some());
    assert!(child.0.wait().unwrap().success());
    let observer = fs::read_to_string(f.0.join("observer-pid")).unwrap();
    let log = fs::read_to_string(trace).unwrap();
    let events: Vec<_> = log
        .lines()
        .filter(|line| line.starts_with(&format!("{observer} ")))
        .collect();
    let killed = events
        .iter()
        .position(|line| line.contains(" group-kill "))
        .unwrap();
    let reaped = events
        .iter()
        .position(|line| line.contains(" reaped "))
        .unwrap();
    assert!(
        killed < reaped,
        "reaping releases the PID for reuse before group cleanup: {events:?}"
    );
}

#[test]
fn recorded_vendor_dhcp_retries_are_adopted_without_spawning_a_duplicate() {
    let f = Fixture::new();
    f.write("proc/123/cmdline", b"udhcpc\0-i\0wlan0\0-x\0hostname:Camera\0-p\0/var/run/udhcpc.pid\0-r\x00172.16.25.200\0--retries\x00100\0");
    assert!(f.host().alive(Proc::DhcpWlan).unwrap());
    assert!(!f.host().alive(Proc::DhcpEth).unwrap());
    f.write("proc/123/cmdline", b"udhcpc\0--retries\0-iwlan0\0");
    assert!(f.host().alive(Proc::DhcpWlan).is_err());
}
