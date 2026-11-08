# Telemetry inputs, real camera text (one derived file marked below)

Copied from `cameras/reolink-e1-zoom/cam/tests/fixtures/telemetry/` (captured read-only from a
Reolink E1 Zoom running `cam serve`) for the readers in `src/telemetry.rs`.

| File | Source |
|---|---|
| `meminfo` | `/proc/meminfo` |
| `status` | `/proc/<pid>/status` of the daemon (the reader uses `/proc/self/status`) |
| `loadavg` | `/proc/loadavg` |
| `uptime` | `/proc/uptime` |
| `stat-10s` | first line of `/proc/stat`, twice, 10 s apart |
| `thermal_zone0_temp` | `/sys/class/thermal/thermal_zone0/temp` (`nvt_thermal`, whole °C) |
| `meminfo-3.10` | `meminfo` without `MemAvailable` and `Percpu`: the layout of a pre-3.14 kernel such as the Mi 360's 3.10 (not a capture) |
