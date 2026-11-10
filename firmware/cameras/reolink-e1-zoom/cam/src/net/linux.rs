//! `net::Host` on the camera's Linux: sysfs, `/proc`, BusyBox `insmod`/`udhcpc` and the Realtek rtl88x2cu
//! module stack, exactly as the vendor's `wifi.sh LOAD` and `start_app` drive them. See docs/hardware/network.md.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::net::Ipv4Addr;
use std::os::unix::{fs::OpenOptionsExt, process::CommandExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use crate::identity::Identity;
use crate::net::{self, Host, Proc};

const GPIO_WIFI: u32 = 133;
const WPA_CTRL_DIR: &str = "/mnt/tmp/wpa_supplicant";
/// Vendor shared libraries required by `wpa_supplicant` (including libssl).
const APP_LIBS: &str = "/mnt/app";
/// Estonian channel plan, what `wifi.sh` picks for `country_code="EE"`.
const CHANNEL_PLAN: &str = "0x3A";

pub struct LinuxHost {
    wpa_conf: String,
    hostname: String,
    root: PathBuf,
    children: HashMap<Proc, Child>,
    powered_off_at: Option<Instant>,
}

impl LinuxHost {
    pub fn new(wpa_conf: impl Into<String>, identity: &Identity) -> Self {
        Self {
            wpa_conf: wpa_conf.into(),
            hostname: identity.hostname().into(),
            root: PathBuf::from("/"),
            children: HashMap::new(),
            powered_off_at: None,
        }
    }

    /// Read-only production preflight; never seeds from the vendor's RAM config.
    pub fn production(identity: &Identity) -> io::Result<Self> {
        Self::production_at(Path::new("/"), identity)
    }

    /// Same OS boundary rooted at an isolated filesystem (commands still execute real processes).
    pub fn production_at(root: &Path, identity: &Identity) -> io::Result<Self> {
        let config = root.join("mnt/para/cam-wifi.conf");
        crate::net::wifi_conf::read_private(&config)?;
        let mut host = Self::new(config.to_string_lossy(), identity);
        host.root = root.to_owned();
        Ok(host)
    }

    /// Exits the process and leaves the network children to init, so the camera stays reachable until the
    /// reboot. Call it last: `process::exit` skips destructors.
    pub fn exit_preserving_network(mut self, status: i32) -> ! {
        for child in self.children.values_mut() {
            let _ = child.try_wait();
        }
        std::process::exit(status)
    }

    /// Stop/reap only children this host started. Adopted vendor processes are never killed.
    /// Does not change interfaces, modules, GPIOs, leases or persistent credentials.
    pub fn shutdown(&mut self) {
        for (_, mut child) in self.children.drain() {
            let _ = stop_child(&mut child);
        }
    }

    fn path(&self, path: &str) -> PathBuf {
        self.root.join(path.trim_start_matches('/'))
    }

    fn gpio(&self, value: &str) -> io::Result<()> {
        let dir = self.path(&format!("/sys/class/gpio/gpio{GPIO_WIFI}"));
        if !dir.exists() {
            // EBUSY from a racing export means someone else exported it, which is fine.
            let _ = fs::write(self.path("/sys/class/gpio/export"), GPIO_WIFI.to_string());
        }
        fs::write(dir.join("direction"), "out")?;
        fs::write(dir.join("value"), value)
    }

    fn insmod(&self, path: &str, params: &str) -> io::Result<()> {
        let name = module_name(path);
        if modules(&self.root)?.contains(&name) {
            return Ok(());
        }
        let mut cmd = Command::new(self.path("/sbin/insmod"));
        cmd.arg(path);
        cmd.args(params.split_whitespace());
        run(cmd)
    }

    fn command(&self, p: Proc) -> Command {
        match p {
            Proc::Wpa => {
                let mut c = Command::new(self.path("/sbin/wpa_supplicant"));
                // libssl.so and friends live in /mnt/app, not in the system library path.
                c.env("LD_LIBRARY_PATH", APP_LIBS);
                c.args(["-Dnl80211", "-iwlan0", "-c", &self.wpa_conf]);
                c
            }
            Proc::DhcpWlan | Proc::DhcpEth => {
                let iface = if p == Proc::DhcpWlan {
                    net::WLAN
                } else {
                    net::ETH
                };
                let mut c = Command::new(self.path("/sbin/udhcpc"));
                c.args([
                    "-f",
                    "-i",
                    iface,
                    "-x",
                    &format!("hostname:{}", self.hostname),
                ]);
                c.args(["-p", &format!("/var/run/udhcpc_{iface}.pid")]);
                c
            }
        }
    }
}

impl Drop for LinuxHost {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn module_name(path: &str) -> String {
    let file = path.rsplit('/').next().unwrap_or(path);
    file.trim_end_matches(".ko").replace('-', "_")
}

fn modules(root: &Path) -> io::Result<Vec<String>> {
    let text = String::from_utf8(read_proc(&root.join("proc/modules"))?)
        .map_err(|_| io::Error::other("invalid module inventory"))?;
    Ok(text
        .lines()
        .filter_map(|l| l.split_whitespace().next().map(str::to_owned))
        .collect())
}

/// Helper stdout/stderr has the same zero-byte budget as long-lived children.
/// Each helper gets two seconds, then its whole process group is killed and its child reaped.
fn run(mut cmd: Command) -> io::Result<()> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(2);
    let result = loop {
        match child_finished(&child) {
            Ok(true) => break Ok(()),
            Ok(false) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(false) => {
                break Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "helper exceeded 2s deadline",
                ));
            }
            Err(e) => break Err(e),
        }
    };
    let status = stop_child(&mut child);
    result?;
    let status = status?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("helper exited: {status}")))
    }
}

/// Observe without reaping: the leader PID must stay reserved until group cleanup.
fn child_finished(child: &Child) -> io::Result<bool> {
    // SAFETY: zero is a valid initial siginfo_t. waitid writes this owned value; WNOWAIT
    // leaves the child waitable, preventing PID/PGID reuse before stop_child signals it.
    unsafe {
        let mut info: libc::siginfo_t = std::mem::zeroed();
        if libc::waitid(
            libc::P_PID,
            child.id() as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        ) < 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(info.si_pid() != 0)
    }
}

fn stop_child(child: &mut Child) -> io::Result<ExitStatus> {
    // SAFETY: every owned child is spawned in its own process group (PGID = child PID),
    // and is not reaped until after this signal, so that ID cannot name a reused group.
    unsafe {
        libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
    }
    let _ = child.kill();
    child.wait()
}

/// Is a process matching `p` running that this host did not start (the vendor stack's)?
fn adopted(root: &Path, p: Proc) -> io::Result<bool> {
    for entry in fs::read_dir(root.join("proc"))? {
        let entry = entry?;
        if entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
            .is_none()
        {
            continue;
        }
        let process = entry.path();
        let identity = (|| {
            let raw = read_proc(&process.join("cmdline"))?;
            if raw.is_empty() {
                let stat = String::from_utf8(read_proc(&process.join("stat"))?)
                    .map_err(|_| io::Error::other("invalid process stat"))?;
                let fields: Vec<_> = stat
                    .rsplit_once(')')
                    .map(|(_, rest)| rest.split_whitespace().collect())
                    .unwrap_or_default();
                let exited = fields.first().is_some_and(|s| matches!(*s, "Z" | "X"));
                let kernel = fields
                    .get(6)
                    .and_then(|s| s.parse::<u64>().ok())
                    .is_some_and(|flags| flags & 0x20_0000 != 0);
                if !exited && !kernel {
                    return Err(io::Error::other("missing surviving process identity"));
                }
            }
            Ok(raw)
        })();
        let raw = match identity {
            Ok(raw) => raw,
            Err(error)
                if error.kind() == io::ErrorKind::NotFound
                    && matches!(fs::metadata(&process), Err(e) if e.kind() == io::ErrorKind::NotFound) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        };
        let args: Vec<&[u8]> = raw.split(|b| *b == 0).collect();
        let exe = args
            .first()
            .map(|a| a.rsplit(|b| *b == b'/').next().unwrap_or(a))
            .unwrap_or_default();
        let (program, interface) = match p {
            Proc::Wpa => (b"wpa_supplicant".as_slice(), b"wlan0".as_slice()),
            Proc::DhcpWlan => (b"udhcpc".as_slice(), b"wlan0".as_slice()),
            Proc::DhcpEth => (b"udhcpc".as_slice(), b"eth0".as_slice()),
        };
        if exe == program && process_interface(&args[1..], p, interface)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Parses only the short-option forms our and the vendor's clients use. Anything else is refused, so an
/// option's value is never mistaken for a second interface option.
fn process_interface(args: &[&[u8]], p: Proc, wanted: &[u8]) -> io::Result<bool> {
    let (values, flags): (&[u8], &[u8]) = if p == Proc::Wpa {
        (b"iDcpbCOPfgGoeI", b"BdsqW")
    } else {
        (b"ixpsrtTAhHVFO", b"fbnqRoSv")
    };
    let invalid = || io::Error::other("unrecognized network client arguments");
    let mut interface = None;
    let mut it = args.iter().copied().filter(|arg| !arg.is_empty());
    while let Some(arg) = it.next() {
        // The vendor runs udhcpc with --retries 100; its value is consumed here, and any other long option
        // is refused rather than read as -i.
        if p != Proc::Wpa && arg == b"--retries" {
            let value = it.next().ok_or_else(invalid)?;
            if value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
                return Err(invalid());
            }
            continue;
        }
        if arg.len() < 2 || arg[0] != b'-' {
            return Err(invalid());
        }
        let option = arg[1];
        if values.contains(&option) {
            let value = if arg.len() > 2 {
                &arg[2..]
            } else {
                it.next().ok_or_else(invalid)?
            };
            if option == b'i' && interface.replace(value).is_some() {
                return Err(invalid());
            }
        } else if !arg[1..].iter().all(|flag| flags.contains(flag)) {
            return Err(invalid());
        }
    }
    interface
        .map(|interface| interface == wanted)
        .ok_or_else(invalid)
}

fn read_proc(path: &Path) -> io::Result<Vec<u8>> {
    crate::files::read_regular(path, 64 * 1024)
}

impl Host for LinuxHost {
    fn power_wifi(&mut self) -> io::Result<()> {
        if self
            .powered_off_at
            .is_some_and(|at| at.elapsed() < Duration::from_millis(500))
        {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Wi-Fi must remain powered off for 500ms",
            ));
        }
        self.gpio("1")
    }

    fn module_loaded(&self, name: &str) -> io::Result<bool> {
        Ok(modules(&self.root)?.iter().any(|m| m == name))
    }

    fn load_wifi_modules(&mut self) -> io::Result<()> {
        let plan = format!("rtw_channel_plan={CHANNEL_PLAN} rtw_max_bss_cnt=100");
        for (path, params) in [
            ("/lib/modules/usb_ko/usb-common.ko", ""),
            ("/lib/modules/usb_ko/usbcore.ko", ""),
            ("/lib/modules/usb_ko/ehci-hcd.ko", ""),
            ("/lib/modules/wifi/firmware_class.ko", ""),
            ("/lib/modules/wifi/rfkill.ko", ""),
            ("/lib/modules/wifi/wifi_8812ko/cfg80211.ko", ""),
            ("/lib/modules/wifi/wifi_8812ko/wlan.ko", plan.as_str()),
        ] {
            self.insmod(path, params)?;
        }
        Ok(())
    }

    fn unload_wifi_modules(&mut self) -> io::Result<()> {
        let mut errors = Vec::new();
        for m in ["wlan", "cfg80211"] {
            let mut c = Command::new(self.path("/sbin/rmmod"));
            c.arg(m);
            if let Err(e) = run(c) {
                errors.push(format!("{m}: {e}"));
            }
        }
        if let Err(e) = self.gpio("0") {
            errors.push(format!("wifi power off: {e}"));
        } else {
            self.powered_off_at = Some(Instant::now());
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(io::Error::other(errors.join("; ")))
        }
    }

    fn load_eth_module(&mut self) -> io::Result<()> {
        let mut c = Command::new(self.path("/sbin/modprobe"));
        c.arg("ntkimethmac");
        run(c)
    }

    fn iface_exists(&self, name: &str) -> bool {
        self.path(&format!("/sys/class/net/{name}")).exists()
    }

    fn iface_is_up(&self, name: &str) -> bool {
        fs::read_to_string(self.path(&format!("/sys/class/net/{name}/flags")))
            .ok()
            .and_then(|f| u32::from_str_radix(f.trim().trim_start_matches("0x"), 16).ok())
            .is_some_and(|f| f & 1 != 0)
    }

    fn iface_up(&mut self, name: &str) -> io::Result<()> {
        let mut c = Command::new(self.path("/sbin/ifconfig"));
        c.args([name, "up"]);
        run(c)
    }

    fn carrier(&self, name: &str) -> bool {
        fs::read_to_string(self.path(&format!("/sys/class/net/{name}/carrier")))
            .is_ok_and(|c| c.trim() == "1")
    }

    fn ipv4(&self, name: &str) -> Option<Ipv4Addr> {
        ipv4_of(name)
    }

    fn clear_address(&mut self, name: &str) -> io::Result<()> {
        let mut c = Command::new(self.path("/sbin/ifconfig"));
        c.args([name, "0.0.0.0"]);
        run(c)
    }

    fn alive(&mut self, p: Proc) -> io::Result<bool> {
        if let Some(child) = self.children.get_mut(&p) {
            match child_finished(child) {
                Ok(true) => {}
                // Retain ownership on observation failure; never duplicate an unaccounted child.
                Ok(false) => return Ok(true),
                Err(error) => return Err(error),
            }
            if let Some(mut child) = self.children.remove(&p) {
                let _ = stop_child(&mut child);
            }
        }
        adopted(&self.root, p)
    }

    fn spawn(&mut self, p: Proc) -> io::Result<()> {
        if self.alive(p)? {
            return Ok(());
        }
        if p == Proc::Wpa {
            fs::create_dir_all(self.path(WPA_CTRL_DIR))?;
        }
        let child = self
            .command(p)
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        self.children.insert(p, child);
        Ok(())
    }

    fn kill(&mut self, p: Proc) {
        if let Some(mut child) = self.children.remove(&p) {
            let _ = stop_child(&mut child);
        }
    }
}

fn ipv4_of(name: &str) -> Option<Ipv4Addr> {
    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills `head` with a list we free with freeifaddrs below; each node is read only
    // while the list is alive, and `ifa_name`/`ifa_addr` are checked for null before use.
    unsafe {
        if libc::getifaddrs(&mut head) != 0 {
            return None;
        }
        let mut found = None;
        let mut cur = head;
        while !cur.is_null() {
            let ifa = &*cur;
            if !ifa.ifa_addr.is_null()
                && i32::from((*ifa.ifa_addr).sa_family) == libc::AF_INET
                && std::ffi::CStr::from_ptr(ifa.ifa_name).to_bytes() == name.as_bytes()
            {
                let sin = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                found = Some(Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr)));
                break;
            }
            cur = ifa.ifa_next;
        }
        libc::freeifaddrs(head);
        found
    }
}

/// Explicit diagnostic seeding only: validate first, publish a complete 0600 file without replacing
/// an existing config, and sync it and its directory. Production never reads the vendor RAM file.
pub fn seed_wpa_conf(ours: &Path, vendor: &Path) -> io::Result<PathBuf> {
    if ours.exists() {
        crate::net::wifi_conf::read_private(ours)?;
        return Ok(ours.to_owned());
    }
    let text = crate::net::wifi_conf::read(vendor)?;
    let temp = ours.with_extension(format!("seed-{}", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)?;
    let result = (|| {
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        match fs::hard_link(&temp, ours) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
            Err(e) => Err(e),
        }
    })();
    let cleanup = fs::remove_file(&temp);
    result?;
    cleanup?;
    fs::File::open(ours.parent().unwrap_or(Path::new(".")))?.sync_all()?;
    crate::net::wifi_conf::read_private(ours)?;
    Ok(ours.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cam-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn first_start_copies_the_vendor_config_and_keeps_it() {
        let d = scratch("seed");
        let (ours, vendor) = (d.join("cam-wifi.conf"), d.join("vendor.conf"));
        fs::write(&vendor, "network={\nssid=\"a\"\npsk=\"password\"\n}").unwrap();
        assert_eq!(seed_wpa_conf(&ours, &vendor).unwrap(), ours);
        assert_eq!(
            fs::read_to_string(&ours).unwrap(),
            "network={\nssid=\"a\"\npsk=\"password\"\n}"
        );
        fs::write(&vendor, "changed").unwrap();
        seed_wpa_conf(&ours, &vendor).unwrap();
        assert_eq!(
            fs::read_to_string(&ours).unwrap(),
            "network={\nssid=\"a\"\npsk=\"password\"\n}"
        );
    }

    #[test]
    fn no_config_anywhere_is_an_error() {
        let d = scratch("none");
        assert!(seed_wpa_conf(&d.join("ours"), &d.join("vendor")).is_err());
    }

    #[test]
    fn wpa_supplicant_finds_the_vendor_libssl() {
        let cmd = LinuxHost::new("/x.conf", &Identity::default()).command(Proc::Wpa);
        let ld = cmd
            .get_envs()
            .find(|(k, _)| *k == "LD_LIBRARY_PATH")
            .and_then(|(_, v)| v);
        assert_eq!(ld, Some(std::ffi::OsStr::new("/mnt/app")));
    }

    #[test]
    fn dhcp_announces_the_camera_id_as_its_hostname() {
        let hall = Identity::new("hall", "Hall").unwrap();
        for p in [Proc::DhcpWlan, Proc::DhcpEth] {
            let cmd = LinuxHost::new("/x.conf", &hall).command(p);
            let args: Vec<_> = cmd.get_args().collect();
            assert!(
                args.windows(2).any(|w| w == ["-x", "hostname:hall"]),
                "{args:?}"
            );
        }
    }

    #[test]
    fn module_names_match_proc_modules() {
        assert_eq!(
            module_name("/lib/modules/usb_ko/usb-common.ko"),
            "usb_common"
        );
        assert_eq!(module_name("/x/wlan.ko"), "wlan");
    }

    #[test]
    fn loopback_has_an_address() {
        let lo = if cfg!(target_os = "linux") {
            "lo"
        } else {
            "lo0"
        };
        assert_eq!(ipv4_of(lo), Some(Ipv4Addr::LOCALHOST));
        assert_eq!(ipv4_of("no-such-iface"), None);
    }

    #[test]
    fn failing_wlan_rmmod_still_removes_cfg80211_and_powers_the_dongle_off() {
        use std::os::unix::fs::PermissionsExt;
        let d = scratch("unload");
        fs::create_dir_all(d.join("sbin")).unwrap();
        let rmmod = d.join("sbin/rmmod");
        let log = d.join("rmmod.log");
        fs::write(
            &rmmod,
            format!(
                "#!/bin/sh\necho \"$1\" >> {}\n[ \"$1\" != wlan ]\n",
                log.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&rmmod, fs::Permissions::from_mode(0o755)).unwrap();
        let gpio = d.join(format!("sys/class/gpio/gpio{GPIO_WIFI}"));
        fs::create_dir_all(&gpio).unwrap();
        fs::write(gpio.join("value"), "1").unwrap();
        let mut host = LinuxHost::new("/x.conf", &Identity::default());
        host.root = d.clone();

        let err = host.unload_wifi_modules().unwrap_err();

        assert!(err.to_string().contains("wlan"), "{err}");
        assert_eq!(fs::read_to_string(&log).unwrap(), "wlan\ncfg80211\n");
        assert_eq!(fs::read_to_string(gpio.join("value")).unwrap(), "0");
        assert!(host.powered_off_at.is_some());
    }
}
