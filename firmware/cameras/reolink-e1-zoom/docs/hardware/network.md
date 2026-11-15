# Network: Wi-Fi, Ethernet fallback and child processes

The stock `device` binary starts the network itself (it spawns `wifi.sh`, `wpa_supplicant` and `udhcpc`), so
with the vendor stack gone the daemon must do it. `cam::net::Supervisor` is a pure reconcile step over the `Host`
trait; `cam::net::linux::LinuxHost` is the camera adapter. The supervisor ticks every 2 s on the network
worker, never on the watchdog feeder.

## Bring-up, as the vendor's `wifi.sh LOAD` does it [V]

1. Export GPIO **133** and drive it high: power for the RTL8822CU USB dongle.
2. `insmod`, in order, skipping modules already loaded: `/lib/modules/usb_ko/usb-common.ko`, `usbcore.ko`,
   `ehci-hcd.ko`, `/lib/modules/wifi/firmware_class.ko`, `rfkill.ko`, `wifi_8812ko/cfg80211.ko`, then
   `wifi_8812ko/wlan.ko rtw_channel_plan=0x3A rtw_max_bss_cnt=100`. `0x3A` is the plan `wifi.sh` picks for country
   code `EE`; it is fixed in `cam/src/net/linux.rs`.
3. Wait up to 10 s for `wlan0`, then `ifconfig wlan0 up`.
4. `iw wlan0 set power_save off`, right after the interface is up, as the vendor's `device` and `netserver`
   binaries do (the command string sits next to their `ifconfig %s up`) [D]. Without it the Realtek driver
   runs its leisure power save (`iw wlan0 get power_save` read `on` on a unit running the daemon [V]): the
   radio parks between beacons whenever traffic looks light, and each wake-up delays what the camera sends. A
   sleeping station still transmits once awake, so this costs latency jitter, not uplink packets. The stock
   `/sbin/iw` is statically linked against libnl and needs no library path. Done once per bring-up of the
   interface, so again after a dongle power cycle; a failing command is reported and retried at most three
   times, 2 s apart (`Event::WifiPowerSaveOff`, logged as `net: … WifiPowerSaveOff`).
5. `wpa_supplicant -Dnl80211 -iwlan0 -c /mnt/para/cam-wifi.conf`, with `LD_LIBRARY_PATH=/mnt/app` (its `libssl`
   lives there).
6. `udhcpc -f -i wlan0 -x hostname:<camera id> -p /var/run/udhcpc_wlan0.pid`.
7. While `wlan0` has had no address for 30 s and the Ethernet cable is up, the same `udhcpc` on `eth0`.

The stock `S25_Net` leaves `eth0` configured as 192.168.0.3/24 at every boot (the vendor stack removes it later; with
the vendor stack gone nothing would). Its connected route is more specific than the one `wlan0` gets on a /23 (or any
network that covers 192.168.0.x), so every packet for the router, the DNS server and every client would leave through
the cable-less `eth0`, while `wlan0` held a perfectly good lease: DHCP worked (it needs no route), then DNS lookups
failed ("Name or service not known"), the MQTT worker stalled, and nothing could reach the camera over the LAN. On a
192.168.1.x or 10.x network the stray route is harmless, which is why it can go unnoticed. The supervisor therefore
removes every IPv4 address from `eth0` once, as soon as the interface exists and no DHCP client of ours or the vendor's
owns it (`Event::EthAddressCleared`, logged as `net: … EthAddressCleared`); the cable fallback below configures `eth0`
again when it is needed. Observed on a unit whose LAN is 192.168.0.0/23: the cold-boot routing table
held both routes and the daemon was reset three times; with the removal the same cold boot logs `EthAddressCleared`,
keeps only the `wlan0` route, resolves the broker, qualifies after 180 s of health and serves RTSP. [V]

Recovery: exited children are respawned after 2 s. A failed load or a missing interface powers the dongle down:
`rmmod wlan`, `rmmod cfg80211`, GPIO 133 low (each attempted even if an earlier one failed), at least 500 ms off
measured from the successful GPIO write, then the sequence again. Wi-Fi and Ethernet retries keep separate clocks.

Every helper (`insmod`, `rmmod`, `modprobe`, `ifconfig`, `iw`) gets 2 s, runs in its own process group, and on
timeout the whole group is killed. Completion is observed with `waitid(WNOWAIT)` so the leader's PID cannot be reused before
the group is signalled. One tick can run seven helpers in a row, which stays inside the 30 s `Network` progress
budget ([../architecture.md](../architecture.md#threads-and-the-watchdog)).

All child stdout and stderr go to `/dev/null`: zero bytes of child logs in RAM or tmpfs, no drain threads. The
supervisor's own events (spawn and load failures, exits, timeouts) are logged, at most 64 lines per boot.

## `cam-wifi.conf`

`/mnt/para/cam-wifi.conf` must be a regular file owned by root, mode 0600, no symlink or hard link, at most
8 KiB. It holds exactly one enabled WPA-PSK network, `network={` and `}` on their own lines, one `key=value` per
line; blank lines and whole-line `#` comments are allowed.

```text
ctrl_interface=/mnt/tmp/wpa_supplicant
network={
	ssid="YOUR_SSID"
	psk="YOUR_PASSPHRASE"
	key_mgmt=WPA-PSK
}
```

- `ssid`: quoted, 1–32 bytes (UTF-8 allowed), or 2–64 hex digits.
- `psk`: quoted, 8–63 printable ASCII characters, or exactly 64 hex digits.
- Quoted values cannot contain quotes, backslashes or control characters: use hex for those.
- Optional in the block: `key_mgmt=WPA-PSK`, `scan_ssid=0|1`, `disabled=0`, a non-negative `priority`,
  `auth_alg=OPEN`, `proto` of WPA and/or RSN, `pairwise`/`group` of CCMP and/or TKIP.
- Optional globals: `ctrl_interface=/mnt/tmp/wpa_supplicant`, `update_config=0`, `ap_scan=1`, `country=EE`.

Anything else is refused: inline comments, duplicate keys, a second network, open, enterprise or SAE-only networks.
This is a deliberately small profile, not a general `wpa_supplicant` parser.

## `cam net`

```
cam net [--check | --seed-vendor]
```

- `--check` validates the file read-only and starts nothing (exit 0 ready, 1 invalid or missing, 2 bad usage).
  Errors name the file and the rule, never its contents. `cam preflight` runs the same check.
- `--seed-vendor` copies the Wi-Fi network the vendor stack rendered into `/mnt/tmp/atheros_config.conf` (only
  present while the vendor stack runs) into `/mnt/para/cam-wifi.conf`, if the vendor file fits the profile above.
  It writes a private temporary file, syncs it, publishes it without replacing an existing file, and syncs the
  directory. An existing valid file is kept; an existing invalid one is reported, never replaced. An interrupted
  run can leave a private `cam-wifi.seed-<pid>` beside it, which is never read. The vendor's encoded `link.cfg` is
  not a source of plain passphrases.
- With no option it runs the supervisor alone, as a diagnostic. It takes the shared runtime lock and its own
  `/mnt/tmp/cam-network.lock`, and refuses while the production daemon runs or after it latched this boot.

## Who owns the children

| Situation | Children it started | Matching processes it found running |
|---|---|---|
| Normal supervision | respawned after exit, never duplicated | recognised by executable and `-i wlan0` / `-iwlan0` (or `eth0`) and adopted, not started again |
| `shutdown()` or drop | process groups killed and reaped | left running |
| Diagnostic exit on SIGINT/SIGTERM | `exit_preserving_network(0)` leaves them to init | left running |
| Production failure | kept alive while media and services stop and the watchdog drives the reboot | left running |
| Terminal production exit | `exit_preserving_network(nonzero)`: the link stays up until the reboot | left running |

`exit_preserving_network` never returns and skips every destructor, so the runtime joins its workers and tears
down media first. Nothing deletes credentials, changes addresses or unloads drivers on the way out, so the camera
stays reachable over SSH until it resets. Process inspection fails safe: an unreadable, oversized or ambiguous
`/proc` entry prevents a spawn rather than risking a duplicate.

Adoption does not serialise two supervisors, and a running vendor `device` would start its own clients, so only
one custom supervisor may run and never beside the vendor stack.
