//! Network supervisor: powers the Wi-Fi dongle, loads its modules, keeps
//! `wpa_supplicant` and `udhcpc` running on `wlan0`, and falls back to `eth0` while Wi-Fi has no address
//! and the cable is up. `Supervisor::tick` is a pure reconcile step over the `Host` trait.

pub mod linux;
pub(crate) mod wifi_conf;

use std::collections::HashMap;
use std::io;
use std::net::Ipv4Addr;
use std::time::Duration;

pub const WLAN: &str = "wlan0";
pub const ETH: &str = "eth0";
const WIFI_POWER_OFF_TIME: Duration = Duration::from_millis(500);

/// The supervised child processes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Proc {
    Wpa,
    DhcpWlan,
    DhcpEth,
}

/// Everything the supervisor does to the machine; the real one is `LinuxHost`, tests use a fake.
pub trait Host {
    /// Export GPIO 133 and drive it high (dongle power). WouldBlock defers a retry until
    /// the adapter's measured powered-off interval has elapsed.
    fn power_wifi(&mut self) -> io::Result<()>;
    fn module_loaded(&self, name: &str) -> io::Result<bool>;
    /// `insmod` the Wi-Fi stack: USB prerequisites, `cfg80211.ko`, `wlan.ko` with the channel plan.
    fn load_wifi_modules(&mut self) -> io::Result<()>;
    /// `rmmod` the Wi-Fi stack and power the dongle off, without sleeping. The supervisor waits at
    /// least 500 ms before calling `power_wifi` to complete the cycle. Report module removal errors.
    fn unload_wifi_modules(&mut self) -> io::Result<()>;
    fn load_eth_module(&mut self) -> io::Result<()>;
    fn iface_exists(&self, name: &str) -> bool;
    fn iface_is_up(&self, name: &str) -> bool;
    fn iface_up(&mut self, name: &str) -> io::Result<()>;
    fn carrier(&self, name: &str) -> bool;
    fn ipv4(&self, name: &str) -> Option<Ipv4Addr>;
    /// Remove every IPv4 address from `name` (and so its connected route).
    fn clear_address(&mut self, name: &str) -> io::Result<()>;
    /// Alive means ours or one adopted from the vendor stack.
    fn alive(&mut self, p: Proc) -> io::Result<bool>;
    fn spawn(&mut self, p: Proc) -> io::Result<()>;
    fn kill(&mut self, p: Proc);
}

#[derive(Debug, Clone)]
pub struct Timing {
    /// Deadline for each power/load attempt, including failed loads, to produce `wlan0`.
    pub iface_wait: Duration,
    /// Pause before respawning a process or retrying a module load.
    pub restart_delay: Duration,
    /// Wi-Fi without an address this long, and a cable present, starts DHCP on `eth0`.
    pub eth_fallback_after: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            iface_wait: Duration::from_secs(10),
            restart_delay: Duration::from_secs(2),
            eth_fallback_after: Duration::from_secs(30),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Event {
    WifiModulesLoaded,
    /// Power is off; the next module retry completes the cycle by powering Wi-Fi on.
    WifiPowerCycled,
    EthModuleLoaded,
    /// The stock boot script's static fallback address on `eth0` was removed, with the route it makes.
    EthAddressCleared,
    Spawned(Proc),
    Killed(Proc),
    Failed(&'static str, String),
}

pub struct Supervisor {
    timing: Timing,
    loaded_at: Option<Duration>,
    power_off: bool,
    load_failed: bool,
    wifi_retry_at: Duration,
    eth_retry_at: Duration,
    eth_cleared: bool,
    eth_clear_retry_at: Duration,
    respawn_at: HashMap<Proc, Duration>,
    wlan_without_ip_since: Option<Duration>,
}

impl Supervisor {
    pub fn new(timing: Timing) -> Self {
        Self {
            timing,
            loaded_at: None,
            power_off: false,
            load_failed: false,
            wifi_retry_at: Duration::ZERO,
            eth_retry_at: Duration::ZERO,
            eth_cleared: false,
            eth_clear_retry_at: Duration::ZERO,
            respawn_at: HashMap::new(),
            wlan_without_ip_since: None,
        }
    }

    /// One reconcile pass at monotonic time `now`.
    pub fn tick(&mut self, host: &mut dyn Host, now: Duration) -> Vec<Event> {
        let mut ev = Vec::new();
        self.wifi(host, now, &mut ev);
        self.eth(host, now, &mut ev);
        ev
    }

    fn wifi(&mut self, host: &mut dyn Host, now: Duration, ev: &mut Vec<Event>) {
        if self.power_off || self.load_failed || !host.iface_exists(WLAN) {
            self.wlan_without_ip_since.get_or_insert(now);
            self.wait_for_dongle(host, now, ev);
            return;
        }
        self.loaded_at = None;
        if !host.iface_is_up(WLAN)
            && let Err(e) = host.iface_up(WLAN)
        {
            self.wlan_without_ip_since.get_or_insert(now);
            ev.push(Event::Failed("wlan0 up", e.to_string()));
            return;
        }
        self.ensure(host, Proc::Wpa, now, ev);
        self.ensure(host, Proc::DhcpWlan, now, ev);
        if host.ipv4(WLAN).is_some() {
            self.wlan_without_ip_since = None;
        } else {
            self.wlan_without_ip_since.get_or_insert(now);
        }
    }

    fn wait_for_dongle(&mut self, host: &mut dyn Host, now: Duration, ev: &mut Vec<Event>) {
        if let Some(at) = self.loaded_at {
            if now.saturating_sub(at) >= self.timing.iface_wait {
                self.loaded_at = None;
                self.power_off = true;
                host.kill(Proc::DhcpWlan);
                host.kill(Proc::Wpa);
                self.wifi_retry_at = now + self.timing.restart_delay.max(WIFI_POWER_OFF_TIME);
                match host.unload_wifi_modules() {
                    Ok(()) => ev.push(Event::WifiPowerCycled),
                    Err(e) => ev.push(Event::Failed("unload wifi", e.to_string())),
                }
            }
            return;
        }
        if now < self.wifi_retry_at {
            return;
        }
        self.wifi_retry_at = now + self.timing.restart_delay;
        let powered = host.power_wifi();
        // A real adapter measures the off interval after slow helper cleanup, not tick entry.
        if powered
            .as_ref()
            .is_err_and(|e| e.kind() == io::ErrorKind::WouldBlock)
        {
            return;
        }
        self.power_off = false;
        self.loaded_at = Some(now);
        let loaded = powered.and_then(|()| host.load_wifi_modules());
        self.load_failed = loaded.is_err();
        match loaded {
            Ok(()) => {
                ev.push(Event::WifiModulesLoaded);
            }
            Err(e) => ev.push(Event::Failed("load wifi", e.to_string())),
        }
    }

    fn ensure(&mut self, host: &mut dyn Host, p: Proc, now: Duration, ev: &mut Vec<Event>) {
        match host.alive(p) {
            Ok(true) => return,
            Err(error) => {
                ev.push(Event::Failed("inspect process", format!("{p:?}: {error}")));
                return;
            }
            Ok(false) => {}
        }
        if self.respawn_at.get(&p).is_some_and(|at| now < *at) {
            return;
        }
        self.respawn_at.insert(p, now + self.timing.restart_delay);
        match host.spawn(p) {
            Ok(()) => ev.push(Event::Spawned(p)),
            Err(e) => ev.push(Event::Failed("spawn", format!("{p:?}: {e}"))),
        }
    }

    /// The stock `S25_Net` leaves `eth0` at 192.168.0.3/24 (the vendor stack removes it later; here nothing
    /// would). Its connected route is more specific than a /23 or /16 Wi-Fi network around 192.168.0.x, so every
    /// packet for the router, DNS server and clients would leave through the cable-less `eth0` and vanish while
    /// `wlan0` held a perfectly good lease. Cleared once, before any DHCP of ours runs on `eth0` (later the
    /// cable fallback configures it again); skipped if a DHCP client already owns it.
    fn clear_stock_eth_address(&mut self, host: &mut dyn Host, now: Duration, ev: &mut Vec<Event>) {
        if self.eth_cleared || now < self.eth_clear_retry_at || !host.iface_exists(ETH) {
            return;
        }
        if host.alive(Proc::DhcpEth).unwrap_or(true) {
            self.eth_cleared = true;
            return;
        }
        if host.ipv4(ETH).is_none() {
            self.eth_cleared = true;
            return;
        }
        match host.clear_address(ETH) {
            Ok(()) => {
                self.eth_cleared = true;
                ev.push(Event::EthAddressCleared);
            }
            Err(e) => {
                self.eth_clear_retry_at = now + self.timing.restart_delay;
                ev.push(Event::Failed("clear eth0 address", e.to_string()));
            }
        }
    }

    fn eth(&mut self, host: &mut dyn Host, now: Duration, ev: &mut Vec<Event>) {
        if !host.iface_exists(ETH) && now >= self.eth_retry_at {
            self.eth_retry_at = now + self.timing.restart_delay;
            match host.module_loaded("ntkimethmac") {
                Ok(false) => {
                    match host.load_eth_module() {
                        Ok(()) => ev.push(Event::EthModuleLoaded),
                        Err(e) => ev.push(Event::Failed("load eth", e.to_string())),
                    }
                    return;
                }
                Ok(true) => {}
                Err(error) => {
                    ev.push(Event::Failed("inspect eth module", error.to_string()));
                    return;
                }
            }
        }
        self.clear_stock_eth_address(host, now, ev);
        let wifi_down_long = self
            .wlan_without_ip_since
            .is_some_and(|s| now.saturating_sub(s) >= self.timing.eth_fallback_after);
        let wanted = wifi_down_long && host.iface_exists(ETH) && host.carrier(ETH);
        if wanted {
            if !host.iface_is_up(ETH)
                && let Err(e) = host.iface_up(ETH)
            {
                ev.push(Event::Failed("eth0 up", e.to_string()));
                return;
            }
            self.ensure(host, Proc::DhcpEth, now, ev);
        } else {
            match host.alive(Proc::DhcpEth) {
                Ok(true) => {
                    host.kill(Proc::DhcpEth);
                    ev.push(Event::Killed(Proc::DhcpEth));
                }
                Ok(false) => {}
                Err(error) => ev.push(Event::Failed("inspect eth DHCP", error.to_string())),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[derive(Default)]
    struct Fake {
        wlan_present: bool,
        wlan_up: bool,
        wlan_ip: Option<Ipv4Addr>,
        eth_present: bool,
        eth_up: bool,
        eth_carrier: bool,
        eth_ip: Option<Ipv4Addr>,
        clear_fails: bool,
        wifi_loaded: bool,
        wifi_powered: bool,
        unload_fails: bool,
        alive: HashSet<Proc>,
        calls: Vec<String>,
        /// When set, `load_wifi_modules` makes `wlan0` appear.
        dongle_works: bool,
    }

    impl Host for Fake {
        fn power_wifi(&mut self) -> io::Result<()> {
            self.calls.push("power".into());
            self.wifi_powered = true;
            Ok(())
        }
        fn module_loaded(&self, name: &str) -> io::Result<bool> {
            Ok(name == "wlan" && self.wifi_loaded || name == "ntkimethmac" && self.eth_present)
        }
        fn load_wifi_modules(&mut self) -> io::Result<()> {
            self.calls.push("load".into());
            self.wifi_loaded = true;
            self.wlan_present = self.dongle_works;
            Ok(())
        }
        fn unload_wifi_modules(&mut self) -> io::Result<()> {
            self.calls.push("unload".into());
            if self.unload_fails {
                return Err(io::Error::other("rmmod wlan failed"));
            }
            self.wifi_loaded = false;
            self.wifi_powered = false;
            Ok(())
        }
        fn load_eth_module(&mut self) -> io::Result<()> {
            self.calls.push("load-eth".into());
            self.eth_present = true;
            Ok(())
        }
        fn iface_exists(&self, name: &str) -> bool {
            if name == WLAN {
                self.wlan_present
            } else {
                self.eth_present
            }
        }
        fn iface_is_up(&self, name: &str) -> bool {
            if name == WLAN {
                self.wlan_up
            } else {
                self.eth_up
            }
        }
        fn iface_up(&mut self, name: &str) -> io::Result<()> {
            self.calls.push(format!("up {name}"));
            if name == WLAN {
                self.wlan_up = true
            } else {
                self.eth_up = true
            }
            Ok(())
        }
        fn carrier(&self, _name: &str) -> bool {
            self.eth_carrier
        }
        fn ipv4(&self, name: &str) -> Option<Ipv4Addr> {
            if name == WLAN {
                self.wlan_ip
            } else {
                self.eth_ip
            }
        }
        fn clear_address(&mut self, name: &str) -> io::Result<()> {
            self.calls.push(format!("clear {name}"));
            if self.clear_fails {
                return Err(io::Error::other("ifconfig failed"));
            }
            if name != WLAN {
                self.eth_ip = None;
            }
            Ok(())
        }
        fn alive(&mut self, p: Proc) -> io::Result<bool> {
            Ok(self.alive.contains(&p))
        }
        fn spawn(&mut self, p: Proc) -> io::Result<()> {
            self.calls.push(format!("spawn {p:?}"));
            self.alive.insert(p);
            Ok(())
        }
        fn kill(&mut self, p: Proc) {
            self.calls.push(format!("kill {p:?}"));
            self.alive.remove(&p);
        }
    }

    fn s(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn healthy_wifi() -> Fake {
        Fake {
            wlan_present: true,
            wlan_up: true,
            wlan_ip: Some(Ipv4Addr::new(192, 168, 4, 118)),
            eth_present: true,
            wifi_loaded: true,
            dongle_works: true,
            ..Fake::default()
        }
    }

    #[test]
    fn cold_start_powers_loads_then_supervises() {
        let mut host = Fake {
            dongle_works: true,
            eth_present: true,
            ..Fake::default()
        };
        let mut sup = Supervisor::new(Timing::default());
        assert_eq!(sup.tick(&mut host, s(0)), [Event::WifiModulesLoaded]);
        assert_eq!(host.calls, ["power", "load"]);
        let ev = sup.tick(&mut host, s(1));
        assert_eq!(
            ev,
            [Event::Spawned(Proc::Wpa), Event::Spawned(Proc::DhcpWlan)]
        );
        assert!(host.calls.contains(&"up wlan0".to_string()));
    }

    #[test]
    fn running_vendor_processes_are_adopted_not_doubled() {
        let mut host = healthy_wifi();
        host.alive.insert(Proc::Wpa);
        host.alive.insert(Proc::DhcpWlan);
        let mut sup = Supervisor::new(Timing::default());
        assert!(sup.tick(&mut host, s(0)).is_empty());
        assert!(host.calls.is_empty());
    }

    #[test]
    fn killed_processes_are_respawned_after_the_delay() {
        let mut host = healthy_wifi();
        let mut sup = Supervisor::new(Timing::default());
        sup.tick(&mut host, s(0));
        host.alive.clear();
        assert!(sup.tick(&mut host, s(1)).is_empty(), "inside restart delay");
        let ev = sup.tick(&mut host, s(3));
        assert_eq!(
            ev,
            [Event::Spawned(Proc::Wpa), Event::Spawned(Proc::DhcpWlan)]
        );
    }

    #[test]
    fn dongle_that_never_enumerates_is_power_cycled_and_retried() {
        let mut host = Fake {
            eth_present: true,
            ..Fake::default()
        };
        let mut sup = Supervisor::new(Timing::default());
        sup.tick(&mut host, s(0));
        assert!(sup.tick(&mut host, s(9)).is_empty());
        assert_eq!(sup.tick(&mut host, s(10)), [Event::WifiPowerCycled]);
        assert!(sup.tick(&mut host, s(11)).is_empty(), "backoff");
        assert_eq!(sup.tick(&mut host, s(12)), [Event::WifiModulesLoaded]);
    }

    #[test]
    fn power_cycle_holds_wifi_off_between_ticks_even_with_no_restart_delay() {
        let mut host = Fake {
            eth_present: true,
            ..Fake::default()
        };
        let mut sup = Supervisor::new(Timing {
            restart_delay: Duration::ZERO,
            ..Timing::default()
        });
        assert_eq!(sup.tick(&mut host, s(0)), [Event::WifiModulesLoaded]);
        assert!(host.wifi_powered);
        assert_eq!(sup.tick(&mut host, s(10)), [Event::WifiPowerCycled]);
        assert!(!host.wifi_powered);
        assert!(sup.tick(&mut host, s(10)).is_empty());
        assert!(sup.tick(&mut host, Duration::from_millis(10499)).is_empty());
        assert!(!host.wifi_powered);
        assert_eq!(
            sup.tick(&mut host, Duration::from_millis(10500)),
            [Event::WifiModulesLoaded]
        );
        assert!(host.wifi_powered);
    }

    #[test]
    fn failed_module_removal_is_reported_and_retry_still_observes_backoff() {
        let mut host = Fake {
            eth_present: true,
            unload_fails: true,
            ..Fake::default()
        };
        let mut sup = Supervisor::new(Timing::default());
        sup.tick(&mut host, s(0));
        assert_eq!(
            sup.tick(&mut host, s(10)),
            [Event::Failed("unload wifi", "rmmod wlan failed".into())]
        );
        assert!(sup.tick(&mut host, s(11)).is_empty());
        assert_eq!(sup.tick(&mut host, s(12)), [Event::WifiModulesLoaded]);
    }

    fn with_stock_eth_address() -> Fake {
        let mut host = healthy_wifi();
        host.eth_ip = Some(Ipv4Addr::new(192, 168, 0, 3));
        host.alive.insert(Proc::Wpa);
        host.alive.insert(Proc::DhcpWlan);
        host
    }

    #[test]
    fn the_stock_static_address_on_eth0_is_removed_once() {
        // Its connected 192.168.0.0/24 route outranks a /23 Wi-Fi network and swallows all LAN traffic.
        let mut host = with_stock_eth_address();
        let mut sup = Supervisor::new(Timing::default());
        assert_eq!(sup.tick(&mut host, s(0)), [Event::EthAddressCleared]);
        assert_eq!(host.eth_ip, None);
        assert!(sup.tick(&mut host, s(2)).is_empty());
        assert_eq!(host.calls.iter().filter(|c| *c == "clear eth0").count(), 1);
        assert!(!host.calls.contains(&"clear wlan0".to_string()));
    }

    #[test]
    fn an_eth0_without_an_address_is_left_alone() {
        let mut host = healthy_wifi();
        host.alive.insert(Proc::Wpa);
        host.alive.insert(Proc::DhcpWlan);
        let mut sup = Supervisor::new(Timing::default());
        assert!(sup.tick(&mut host, s(0)).is_empty());
        assert!(!host.calls.iter().any(|c| c.starts_with("clear")));
    }

    #[test]
    fn eth0_is_left_alone_while_a_dhcp_client_owns_it() {
        let mut host = with_stock_eth_address();
        host.alive.insert(Proc::DhcpEth);
        host.eth_carrier = true;
        host.wlan_ip = None;
        let mut sup = Supervisor::new(Timing::default());
        sup.tick(&mut host, s(0));
        assert!(!host.calls.iter().any(|c| c.starts_with("clear")));
        assert_eq!(host.eth_ip, Some(Ipv4Addr::new(192, 168, 0, 3)));
    }

    #[test]
    fn a_failed_clear_is_reported_and_retried_after_the_restart_delay() {
        let mut host = with_stock_eth_address();
        host.clear_fails = true;
        let mut sup = Supervisor::new(Timing::default());
        let first = sup.tick(&mut host, s(0));
        assert!(matches!(
            first.as_slice(),
            [Event::Failed("clear eth0 address", _)]
        ));
        // not again before the delay
        assert!(
            sup.tick(&mut host, s(0) + Duration::from_millis(500))
                .is_empty()
        );
        host.clear_fails = false;
        let retry = sup.tick(&mut host, s(0) + Timing::default().restart_delay);
        assert_eq!(retry, [Event::EthAddressCleared]);
        assert_eq!(host.eth_ip, None);
    }

    #[test]
    fn the_cable_fallback_still_works_after_the_stock_address_is_cleared() {
        let mut host = with_stock_eth_address();
        host.wlan_ip = None;
        host.eth_carrier = true;
        let mut sup = Supervisor::new(Timing::default());
        assert_eq!(sup.tick(&mut host, s(0)), [Event::EthAddressCleared]);
        assert!(sup.tick(&mut host, s(29)).is_empty());
        assert_eq!(sup.tick(&mut host, s(30)), [Event::Spawned(Proc::DhcpEth)]);
    }

    #[test]
    fn eth0_takes_over_only_after_the_grace_period_with_a_cable() {
        let mut host = healthy_wifi();
        host.wlan_ip = None;
        host.eth_carrier = true;
        host.alive.insert(Proc::Wpa);
        host.alive.insert(Proc::DhcpWlan);
        let mut sup = Supervisor::new(Timing::default());
        assert!(sup.tick(&mut host, s(0)).is_empty());
        assert!(sup.tick(&mut host, s(29)).is_empty());
        assert_eq!(sup.tick(&mut host, s(30)), [Event::Spawned(Proc::DhcpEth)]);
        assert!(host.calls.contains(&"up eth0".to_string()));
    }

    #[test]
    fn eth0_without_a_cable_is_never_started() {
        let mut host = healthy_wifi();
        host.wlan_ip = None;
        host.alive.insert(Proc::Wpa);
        host.alive.insert(Proc::DhcpWlan);
        let mut sup = Supervisor::new(Timing::default());
        sup.tick(&mut host, s(0));
        assert!(sup.tick(&mut host, s(100)).is_empty());
    }

    #[test]
    fn eth0_dhcp_stops_when_wifi_returns() {
        let mut host = healthy_wifi();
        host.wlan_ip = None;
        host.eth_carrier = true;
        host.alive.insert(Proc::Wpa);
        host.alive.insert(Proc::DhcpWlan);
        let mut sup = Supervisor::new(Timing::default());
        sup.tick(&mut host, s(0));
        sup.tick(&mut host, s(30));
        host.wlan_ip = Some(Ipv4Addr::new(192, 168, 4, 118));
        assert_eq!(sup.tick(&mut host, s(31)), [Event::Killed(Proc::DhcpEth)]);
    }

    #[test]
    fn module_retries_for_one_interface_do_not_delay_the_other() {
        let mut host = Fake::default();
        let mut sup = Supervisor::new(Timing::default());
        assert_eq!(
            sup.tick(&mut host, s(0)),
            [Event::WifiModulesLoaded, Event::EthModuleLoaded]
        );
        assert_eq!(sup.tick(&mut host, s(10)), [Event::WifiPowerCycled]);
        host.eth_present = false;
        assert_eq!(sup.tick(&mut host, s(11)), [Event::EthModuleLoaded]);
        assert_eq!(sup.tick(&mut host, s(12)), [Event::WifiModulesLoaded]);
    }

    #[test]
    fn missing_eth_module_is_loaded() {
        let mut host = healthy_wifi();
        host.eth_present = false;
        let mut sup = Supervisor::new(Timing::default());
        host.alive.insert(Proc::Wpa);
        host.alive.insert(Proc::DhcpWlan);
        assert_eq!(sup.tick(&mut host, s(0)), [Event::EthModuleLoaded]);
    }
}
