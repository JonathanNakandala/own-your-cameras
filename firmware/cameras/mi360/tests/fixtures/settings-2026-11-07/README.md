# Mi 360 settings files, format as of 2026-11-07

Fixtures for `src/settings.rs`: settings documents in the format the camera wrote at
`/mnt/data/mi360/settings.json` before the move to the shared `camera_transport::settings`
(`{"version":1,"values":{…}}`, compact JSON, keys sorted, no trailing newline).

Source: written by hand in exactly the byte format of the former `settings::Store::save`, as its
round-trip test encoded it; the values are catalog defaults and test values, not read from any device.

- `settings.json`: a saved file; it must load unchanged and save back byte for byte.
- `settings-invalid-fields.json`: out-of-range values and a key no longer in the catalog; the valid
  fields load, the others fall back and are named.
