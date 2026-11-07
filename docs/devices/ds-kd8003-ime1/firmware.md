# Build and use a DS-KD8003-IME1 startup hook

[DS-KD8003-IME1 overview](README.md)

**Supported:** DS-KD8003-IME1, original non-B hardware, V2.2.62 build 230204.
The builder changes only encrypted `/start.sh`. No application or credentials are
bundled. [Format and evidence](firmware-format.md).

The [installer](../../../firmware/cameras/ds-kd8003-ime1/docs/install.md) does all of the steps below
by itself. This page is the manual path.

## 1. Prepare

- Install Python 3.14 (the tested version).
- The input is either the unmodified V2.2.62 build 230204 DAV or a read of the
  panel's firmware partition, from which the builder rebuilds that DAV:
  `printf 'cat /dev/mtd1\nexit\n' | ssh -T root@CAMERA_HOST > mtd1.bin`
  (the [non-PTY root shell](README.md#get-a-root-shell); a banner the shell prints
  before the data is skipped). Other builds are rejected.
- Open a terminal at this repository's root.

Required stock DAV SHA-256 (checked on the DAV, or on the one rebuilt from the read):

```text
f5b080741e4b1360c13d0f46abd6af1b6df7e07cbf62c6a7c6435203bb6d107d
```

## 2. Build

Replace `STOCK_DAV` with your DAV or `mtd1.bin`:

```sh
python3.14 -m venv .venv
.venv/bin/python -m pip install -r tools/ds-kd8003-ime1/requirements.txt
.venv/bin/python tools/ds-kd8003-ime1/build.py STOCK_DAV \
  --output build/ds-kd8003-ime1-hook
```

Dependency installation needs network access. The builder itself is offline and
never contacts a camera. Use a new output directory for every attempt, including
retries after failure. Python `-O` is refused because it disables validation.

## 3. Inspect

Success prints hashes, `Changed files: ['/start.sh']`, and flash headroom.
For the unchanged public hook, headroom is **1,028,096 bytes**.

| Output in `build/ds-kd8003-ime1-hook/` | Check |
|---|---|
| `stock-roundtrip.dav` | Exactly matches the input DAV |
| `ds-kd8003-ime1-2.2.62-hook.dav` | Candidate; record its printed SHA-256 |
| `manifest.json` | `only_changed_file` is `["/start.sh"]`; roundtrip is `true` |
| `start-stock.sh`, `start-candidate.sh` | Review the appended hook |
| `stock.cramfs`, `candidate.cramfs` | For independent filesystem checks |

On Linux with `fsck.cramfs` installed, also run:

```sh
fsck.cramfs -v build/ds-kd8003-ime1-hook/stock.cramfs
fsck.cramfs -v build/ds-kd8003-ime1-hook/candidate.cramfs
```

Both must pass. Keep all generated files local; `build/` is ignored.

## 4. Install — separate device operation

**Flashing changes the device and may make it unbootable.** An upload is not a dry
run. Before proceeding, retain stock firmware and a device configuration backup,
and establish a recovery plan. Dual-bank, serial and power-loss recovery have
not been verified. Offline checks do not prove safe flashing.

The original image was installed through this ISAPI endpoint. Replace
`CAMERA_HOST` with your device's address; `curl` prompts for its admin password:

```sh
curl --fail --show-error --digest --user admin --request PUT \
  --header 'Content-Type: application/octet-stream' \
  --data-binary @build/ds-kd8003-ime1-hook/ds-kd8003-ime1-2.2.62-hook.dav \
  'http://CAMERA_HOST/ISAPI/System/updateFirmware'
```

If `curl` reports an error, stop and inspect the device; do not retry blindly.
An HTTP success alone does not prove installation. Allow the update/reboot to
finish, then check the marker below. Do not interrupt power. Reflashing the retained
stock DAV through the same endpoint was the rollback plan, if the updater remains
reachable; it is not a recovery guarantee.

## 5. Check the hook

Enable maintenance SSH and use the [non-PTY root-shell method](README.md#get-a-root-shell).
After installation and reboot:

```sh
ssh -T root@CAMERA_HOST <<'SH'
cat /home/app/custom/boot-marker
exit
SH
```

Expected: `ds-kd8003-ime1-custom-boot-v1`. Check video, audio and access control
separately. A marker alone does not prove those functions work.

## 6. Use the hook

The hook runs `/home/config/custom/start.sh` at every boot when `/home/config/custom/enabled`
exists and `/home/config/custom/disabled` does not. The
[native application](../../../firmware/cameras/ds-kd8003-ime1/docs/install.md) installs its
supervisor there. Marker and log files are in RAM and disappear at reboot.

[Hook source](../../../tools/ds-kd8003-ime1/boot-hook.sh) · [Technical reference](firmware-format.md)
