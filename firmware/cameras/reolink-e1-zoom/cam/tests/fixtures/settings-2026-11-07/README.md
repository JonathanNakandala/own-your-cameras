# Reolink settings files, as written before the shared settings store (2026-11-07)

`cam-settings.toml` documents in the format the daemon wrote up to commit `fe19c6a`. Generated offline by
that commit's own `settings::Store` writer, in a host test, from the stock-defaults text its tests use; no
camera was read.

- `first-run.toml`: every setting at its first-run value (stock `mirror="1"`).
- `changed.toml`: the same after eight changes through MQTT payloads (`mic_volume 55`, `aec ON`,
  `ir night`, `main_resolution 2592x1944`, `ptz_speed 600`, `isp_anti_flicker 60 Hz`, `zoom_speed 40`,
  `mirror OFF`).
- `hand-edited.toml`: a partial file with an unknown key, as `settings/tests.rs` encodes a hand edit.
