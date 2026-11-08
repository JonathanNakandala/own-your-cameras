# Telemetry inputs, real camera text

Captured read-only over ssh from a camera running `cam serve` (PID 537), for the parsers in
`cam/src/telemetry.rs`. Only the AP address and SSID in `iw-link` are redacted.

| File | Source |
|---|---|
| `meminfo` | `/proc/meminfo` |
| `status` | `/proc/537/status` (the daemon; the parser reads `/proc/self/status`) |
| `loadavg` | `/proc/loadavg` |
| `uptime` | `/proc/uptime` |
| `stat-10s` | first line of `/proc/stat`, twice, 10 s apart |
| `thermal_zone0_temp` | `/sys/class/thermal/thermal_zone0/temp` (`type` = `nvt_thermal`, whole °C) |
| `iw-link` | `iw dev wlan0 link` (the only real dBm reading on this unit) |
