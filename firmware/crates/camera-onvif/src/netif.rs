//! Runtime network identity: the interface, MAC and prefix length ONVIF reports, plus DNS servers and
//! the hostname. Read from the running device (Linux `getifaddrs`, `/sys/class/net`, `/etc/resolv.conf`);
//! nothing device-specific is ever compiled in.

use std::ffi::CStr;
use std::net::Ipv4Addr;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interface {
    pub name: String,
    pub ipv4: Ipv4Addr,
    pub prefix_len: u8,
    pub mac: Option<[u8; 6]>,
}

/// `aa:bb:cc:dd:ee:ff`; the all-zero address counts as none.
pub fn parse_mac(text: &str) -> Option<[u8; 6]> {
    let mut mac = [0u8; 6];
    let mut parts = text.trim().split(':');
    for b in &mut mac {
        *b = u8::from_str_radix(parts.next()?, 16).ok()?;
    }
    (parts.next().is_none() && mac != [0; 6]).then_some(mac)
}

fn mac_of(sys: &Path, name: &str) -> Option<[u8; 6]> {
    parse_mac(&std::fs::read_to_string(sys.join(name).join("address")).ok()?)
}

/// IPv4 interfaces that are up and not loopback, in kernel order.
pub fn interfaces() -> Vec<Interface> {
    interfaces_with(Path::new("/sys/class/net"))
}

fn interfaces_with(sys: &Path) -> Vec<Interface> {
    let mut out = Vec::new();
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills `list` with a heap list we free below with freeifaddrs; on failure it
    // returns -1 and leaves nothing to free.
    if unsafe { libc::getifaddrs(&mut list) } != 0 {
        return out;
    }
    let mut at = list;
    while !at.is_null() {
        // SAFETY: `at` is a node of the list getifaddrs returned, valid until freeifaddrs.
        let ifa = unsafe { &*at };
        at = ifa.ifa_next;
        let flags = ifa.ifa_flags as libc::c_int;
        if ifa.ifa_addr.is_null()
            || ifa.ifa_name.is_null()
            || flags & libc::IFF_UP == 0
            || flags & libc::IFF_LOOPBACK != 0
        {
            continue;
        }
        // SAFETY: ifa_addr is non-null and points to a sockaddr whose family we check before casting.
        if i32::from(unsafe { (*ifa.ifa_addr).sa_family }) != libc::AF_INET {
            continue;
        }
        // SAFETY: AF_INET addresses are sockaddr_in; the netmask, when present, has the same family.
        let (ip, mask) = unsafe {
            let addr = &*(ifa.ifa_addr as *const libc::sockaddr_in);
            let mask = if ifa.ifa_netmask.is_null() {
                0
            } else {
                (*(ifa.ifa_netmask as *const libc::sockaddr_in))
                    .sin_addr
                    .s_addr
            };
            (u32::from_be(addr.sin_addr.s_addr), u32::from_be(mask))
        };
        // SAFETY: ifa_name is a non-null NUL-terminated string owned by the list.
        let name = unsafe { CStr::from_ptr(ifa.ifa_name) }
            .to_string_lossy()
            .into_owned();
        out.push(Interface {
            mac: mac_of(sys, &name),
            name,
            ipv4: Ipv4Addr::from(ip),
            prefix_len: mask.count_ones() as u8,
        });
    }
    // SAFETY: `list` came from a successful getifaddrs and is freed exactly once.
    unsafe { libc::freeifaddrs(list) };
    out
}

/// `nameserver` lines of a resolv.conf.
pub fn parse_resolv(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| l.trim().strip_prefix("nameserver"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

pub fn dns() -> Vec<String> {
    std::fs::read_to_string("/etc/resolv.conf")
        .map(|t| parse_resolv(&t))
        .unwrap_or_default()
}

pub fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|h| h.trim().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macs_and_resolv_conf_parse() {
        assert_eq!(
            parse_mac("02:00:5e:10:00:01\n"),
            Some([2, 0, 0x5e, 0x10, 0, 1])
        );
        assert_eq!(parse_mac("00:00:00:00:00:00"), None);
        assert_eq!(parse_mac("02:00:5e:10:00"), None);
        assert_eq!(parse_mac("02:00:5e:10:00:01:ff"), None);
        assert_eq!(
            parse_resolv(
                "# comment\nnameserver 192.0.2.1\nsearch example\nnameserver  192.0.2.2\n"
            ),
            ["192.0.2.1", "192.0.2.2"]
        );
    }

    #[test]
    fn interfaces_skip_loopback() {
        // Whatever this host has, loopback is never reported.
        assert!(interfaces().iter().all(|i| !i.ipv4.is_loopback()));
    }
}
