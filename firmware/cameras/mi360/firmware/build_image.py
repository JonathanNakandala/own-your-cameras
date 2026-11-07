#!/usr/bin/env python3
"""Offline, exact-base B_USRFS replacement; never flashes or activates an image."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shlex
import struct
import subprocess
import mtd_writer

HERE = Path(__file__).resolve().parent
REPO = HERE.parent
PUBLIC_ROOT = HERE.parents[3]
CAMERA_UPDATE_SHA256 = "9da21170563d0b0f633abb8b0b51165697bb2fad826d71f7a547608b0b4bebfe"
OTA_QUIESCE_ORIGINAL = b"""quiesce_ota() {
    if [ -d /tmp/perp/miio_ota ]; then
        PERP_BASE=/tmp/perp perpctl d miio_ota || fail 'Cannot disable vendor OTA supervisor.'
    fi
    for attempt in 1 2 3 4 5; do
        if ! pidof miio_ota >/dev/null; then return; fi
        sleep 1
    done
    fail 'Vendor OTA is still running; refusing concurrent flash access.'
}
"""
OTA_QUIESCE_REPLACEMENT = OTA_QUIESCE_ORIGINAL.replace(
    b"        PERP_BASE=/tmp/perp perpctl d miio_ota || fail 'Cannot disable vendor OTA supervisor.'\n",
    b"        # An unactivated service directory returns111; verify actual exit below.\n"
    b"        PERP_BASE=/tmp/perp perpctl d miio_ota || :\n",
)
REPLACED_STOCK = frozenset({"etc/init.d/S00init", "etc/init.d/S60service", "bin/camera-update", "bin/camera-mtd-write"})
RETAINED_STOCK_COUNT = 711
START, SIZE, FLASH_SIZE = 0xAA0000, 0x470000, 0x1000000
TIMESTAMP = 1792972800
MARKER = b"# START OF DATA - DO NOT MODIFY\n#\n"
KEEP_INIT = frozenset({
    "S00init", "S01update_mac", "S01logging", "S09ingenic_ko", "S10dynamic_ko",
    "S11mdev", "S15watchdog", "S20urandom", "S21urandom_chmod", "S50perp",
    "S60service", "S99zz_rootssh",
})
# Reviewed names only: never globs against unknown vendor/helper files.
REMOVE_BINS = frozenset({
    "fetch_av", "imi_alarm", "imi_ali.sh", "imi_crypto_v2", "imi_encrypto",
    "ingenic_algo", "miio_agent", "miio_client", "miio_client_helper_nomqtt.sh",
    "miio_cloudv2", "miio_devicekit", "miio_log", "miio_miss", "miio_nas",
    "miio_nas_syncer", "miio_ota", "miio_pincode", "miio_record", "miio_recv_line",
    "miio_sdcard", "miio_send_line", "mortoxc", "mortoxd", "network_governor.sh",
    "pre-ota.sh", "rsa_verify", "dual_system_check.sh", "fsck_check.sh",
})
# Reviewed vendor libraries nothing in the image loads (Alex, 2026-11-07): the person-detection stack, the
# cloud streaming library, the vendor Opus (cam links its own) and the stock A/V board library. cam dlopens
# only libimp and libalog (libimp loads libaudioProcess); the kept stock programs' DT_NEEDED closures and
# ROOTFS's (hostapd/wpa_supplicant need libnl.so, curl needs libcurl) never reach them. Removing them makes
# room for the firmware in this slot only; the vendor firmware keeps them.
REMOVE_LIBS = frozenset({
    "libboardav.so", "libboardav.so.1", "libboardav.so.1.0.0",
    "libgomp.so", "libgomp.so.1", "libgomp.so.1.0.0",
    "libjzdl.so", "libmiss.so",
    "libmxu_contrib.so", "libmxu_core.so", "libmxu_imgproc.so", "libmxu_merge.so",
    "libmxu_objdetect.so", "libmxu_video.so",
    "libopus.so", "libopus.so.0", "libopus.so.0.5.4",
    "libpersonDet_inf.so",
})
PARTITIONS = (
    ("BOOT", 0, 0x50000), ("ROOTFS", 0x50000, 0x2D0000),
    ("A_KERNEL", 0x2D0000, 0x480000), ("A_USRFS", 0x480000, 0x8F0000),
    ("B_KERNEL", 0x8F0000, START), ("B_USRFS", START, START + SIZE),
    ("DATA", 0xF10000, 0xF50000), ("SOUND", 0xF50000, 0xFF0000),
    ("FACTORY", 0xFF0000, FLASH_SIZE),
)


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def run(*args: str) -> bytes:
    result = subprocess.run(args, check=False, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if result.returncode:
        raise ValueError(f"{args[0]} failed ({result.returncode}): {result.stderr.decode(errors='replace')[:2000]}")
    return result.stdout


def pseudo_records(pseudo: bytes) -> tuple[dict[str, dict], list[str], bytes]:
    """Inspect unsquashfs -pf metadata/data without trusting host extraction modes."""
    header, data = pseudo.split(MARKER, 1)
    manifest, lines = {}, header.decode().splitlines()
    for line in lines:
        if not line or line.startswith("#"):
            continue
        fields = line.split()
        if len(fields) < 6:
            raise ValueError("invalid pseudo record")
        name, kind, timestamp, mode, uid, gid = fields[:6]
        # Exact stock manifest has no escaped/space-bearing paths. Reject rather
        # than silently change their spelling or let a pseudo command be injected.
        if name != "/" and (name.startswith("/") or any(p in ("", ".", "..") for p in name.split("/"))):
            raise ValueError(f"unsafe pseudo path {name!r}")
        if any(not (c.isalnum() or c in "_./-+@=") for c in name):
            raise ValueError(f"unsupported pseudo path {name!r}")
        item = {"type": kind, "mtime": int(timestamp), "mode": mode, "uid": int(uid), "gid": int(gid)}
        if kind == "R":
            size, offset, sparse = map(int, fields[6:9])
            if size < 0 or offset < 0 or sparse or offset + size > len(data):
                raise ValueError("unsupported/invalid pseudo file data")
            item.update(size=size, sha256=sha(data[offset:offset + size]))
        elif kind == "S":
            item["target"] = " ".join(fields[6:])
        elif kind in ("D", "C", "B", "I", "L"):
            item["extra"] = fields[6:]
        else:
            raise ValueError(f"unsupported pseudo type {kind}")
        if name in manifest:
            raise ValueError(f"duplicate pseudo path {name}")
        manifest[name] = item
    return manifest, lines, data


def contents(pseudo: bytes, wanted: str) -> bytes:
    _, lines, data = pseudo_records(pseudo)
    fields = next(line.split() for line in lines if line.split()[:1] == [wanted])
    if fields[1] != "R":
        raise ValueError(f"expected regular file {wanted}")
    size, offset = map(int, fields[6:8])
    return data[offset:offset + size]


def check_cam(elf: bytes) -> dict:
    if len(elf) < 52 or elf[:7] != b"\x7fELF\x01\x01\x01":
        raise ValueError("cam must be ELF32 little-endian")
    if struct.unpack_from("<HHI", elf, 16) != (2, 8, 1):
        raise ValueError("cam must be MIPS ET_EXEC")
    flags = struct.unpack_from("<I", elf, 36)[0]
    if flags & 0xF000F000 != 0x70001000:
        raise ValueError("cam must use MIPS32r2 O32 ABI")
    offset = struct.unpack_from("<I", elf, 28)[0]
    width, count = struct.unpack_from("<HH", elf, 42)
    if width != 32 or not count or count > 64 or offset + width * count > len(elf):
        raise ValueError("invalid cam program headers")
    interpreters = []
    for i in range(count):
        kind, start, _, _, length, *_ = struct.unpack_from("<8I", elf, offset + i * width)
        if start + length > len(elf):
            raise ValueError("invalid cam program segment")
        if kind == 3:
            interpreters.append(elf[start:start + length])
    if interpreters != [b"/lib/ld-uClibc.so.0\0"]:
        raise ValueError("cam must use the shipped /lib/ld-uClibc.so.0 loader")
    return {"sha256": sha(elf), "bytes": len(elf), "elf_flags": hex(flags), "interpreter": "/lib/ld-uClibc.so.0"}


def init_script(original: bytes) -> bytes:
    if original.count(b"    telnetd &\n") != 1:
        raise ValueError("stock S00init telnet instruction differs")
    result = original.replace(b"    telnetd &\n", b"    # Telnet disabled; authenticated root SSH is retained.\n")
    marker = b"\t#set ircult to day\n"
    if result.count(marker) != 1:
        raise ValueError("stock S00init IR-day marker differs")
    gpio = b"""\t# Qualified own-board prerequisites; never re-export existing GPIOs.
\tfor pin in 9 49 57; do
\t\tif [ ! -d /sys/class/gpio/gpio$pin ]; then
\t\t\techo $pin > /sys/class/gpio/export
\t\tfi
\t\tif [ $pin = 57 ]; then
\t\t\techo in > /sys/class/gpio/gpio$pin/direction
\t\telse
\t\t\techo low > /sys/class/gpio/gpio$pin/direction
\t\tfi
\tdone

"""
    # sysfs direction="low" atomically sets output-low, avoiding a high glitch.
    return result.replace(marker, gpio + marker)


def camera_update_script(original: bytes, writer_path: str = "/mnt/usr/bin/camera-mtd-write") -> bytes:
    """Retain updater guards; pin the qualified writer and preflight both targets."""
    if sha(original) != CAMERA_UPDATE_SHA256:
        raise ValueError("stock camera-update hash differs")
    if original.count(OTA_QUIESCE_ORIGINAL) != 1:
        raise ValueError("stock camera-update OTA quiesce block differs")
    parts = writer_path.split("/")
    if (writer_path != "/mnt/usr/bin/camera-mtd-write" and
        (not writer_path.startswith("/tmp/") or len(parts) < 4 or parts[-1] != "camera-mtd-write")):
        raise ValueError("writer path must be the packaged path or a private RAM directory")
    if any(not part or part in (".", "..") or not all(c.isascii() and (c.isalnum() or c in "_-." ) for c in part)
           for part in parts[1:]):
        raise ValueError("unsafe writer path")
    result = original.replace(OTA_QUIESCE_ORIGINAL, OTA_QUIESCE_REPLACEMENT)
    for operation in ("check", "write"):
        for kind, argument in (("kernel", "$2"), ("usrfs", "$3")):
            old = f'        camera-mtd-write {operation} {kind} "{argument}"\n'.encode()
            if result.count(old) != 1:
                raise ValueError("stock camera-update writer call differs")
            command = "plan" if operation == "check" else "write"
            new = f'        {shlex.quote(writer_path)} {command} {kind} "{argument}"\n'.encode()
            result = result.replace(old, new, 1)
    return result


def removed_paths(before: dict) -> set[str]:
    init = {name for name in before if name.startswith("etc/init.d/S") and name.rsplit("/", 1)[-1] not in KEEP_INIT}
    bins = {f"bin/{name}" for name in REMOVE_BINS}
    libs = {f"lib/{name}" for name in REMOVE_LIBS}
    missing = (bins | libs) - before.keys()
    if missing:
        raise ValueError(f"reviewed removal names missing from exact base: {sorted(missing)}")
    return init | bins | libs


def retained_scope(before: dict, removed: set[str], files: dict) -> tuple[set[str], set[str]]:
    replaced = before.keys() & files.keys()
    retained = before.keys() - removed - replaced
    if replaced != REPLACED_STOCK or len(retained) != RETAINED_STOCK_COUNT:
        raise ValueError(f"expected exactly four stock replacements and {RETAINED_STOCK_COUNT} retained paths")
    return replaced, retained


def patch_flash(base: bytes, payload: bytes) -> tuple[bytes, bytes]:
    if len(base) != FLASH_SIZE or len(payload) > SIZE:
        raise ValueError("flash/payload size violates fixed partition boundary")
    padded = payload.ljust(SIZE, b"\xff")
    image = base[:START] + padded + base[START + SIZE:]
    if image[:START] != base[:START] or image[START + SIZE:] != base[START + SIZE:]:
        raise ValueError("protected bytes changed")
    return padded, image


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--original-dump", required=True, type=Path)
    parser.add_argument("--expected-original-sha256", required=True)
    parser.add_argument("--ssh-binary", required=True, type=Path)
    parser.add_argument("--bootstrap-writer", required=True, type=Path)
    parser.add_argument("--authorized-key", required=True, type=Path)
    parser.add_argument("--cam", required=True, type=Path)
    parser.add_argument("--writer", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    os.umask(0o077)
    import bootstrap
    bootstrap.check_original(args.original_dump.read_bytes(), args.expected_original_sha256)
    cam_data = args.cam.read_bytes()
    cam_info = check_cam(cam_data)
    writer_info = mtd_writer.verify_artifact(args.writer)
    writer_data = args.writer.read_bytes()
    if sha(writer_data) != writer_info["binary"]["sha256"]:
        raise ValueError("writer changed after provenance verification")
    out = args.output.resolve()
    if out.is_relative_to(PUBLIC_ROOT.resolve()):
        raise ValueError("output must be a new private directory outside this repository")
    out.mkdir(mode=0o700)
    base, bootstrap_report = bootstrap.prepare(args, out)
    versions = {}
    for tool in ("mksquashfs", "unsquashfs"):
        # unsquashfs4.7.5 prints its version and exits1; extraction still must0.
        version = subprocess.run([tool, "-version"], check=False, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        result = version.stdout.decode().splitlines()[0]
        if "version 4.7.5 " not in result:
            raise ValueError(f"reproducible recipe requires {tool}4.7.5, got {result}")
        versions[tool] = result
    source = out / "base-usrfs.bin"
    source.write_bytes(base[START:START + SIZE])
    pseudo_path = out / "base.pseudo"
    run("unsquashfs", "-no-progress", "-pf", str(pseudo_path), str(source))
    pseudo = pseudo_path.read_bytes()
    before, lines, data = pseudo_records(pseudo)
    removed = removed_paths(before)
    inputs = out / "inputs"
    inputs.mkdir(mode=0o700)
    # Copy mutable source inputs once: hashes and emitted bytes share one snapshot.
    copied_cam = inputs / "cam"
    copied_cam.write_bytes(cam_data)
    files: dict[str, tuple[Path, str]] = {"bin/cam": (copied_cam, "755")}
    if sha(contents(pseudo, "bin/camera-mtd-write")) not in mtd_writer.BASE_BINARY_SHA256S:
        raise ValueError("stock MTD-writer binary differs")
    copied_writer = inputs / "camera-mtd-write"
    copied_writer.write_bytes(writer_data)
    files["bin/camera-mtd-write"] = (copied_writer, before["bin/camera-mtd-write"]["mode"])
    for name in ("boot-policy.sh", "network-start.sh", "launcher.sh"):
        copy = inputs / name
        copy.write_bytes((HERE / name).read_bytes())
        files[f"etc/mi360/{name}"] = (copy, "700")
    for name in ("S60service", "S99mi360_network", "S99zzz_mi360"):
        copy = inputs / name
        copy.write_bytes((HERE / name).read_bytes())
        files[f"etc/init.d/{name}"] = (copy, "755")
    for name in ("recover", "dhcp", "wpa-event", "ap"):
        copy = inputs / f"network-{name}.sh"
        copy.write_bytes((REPO / f"scripts/network-{name}.sh").read_bytes())
        files[f"etc/mi360/network/{name}.sh"] = (copy, "700")
    s00 = inputs / "S00init"
    s00.write_bytes(init_script(contents(pseudo, "etc/init.d/S00init")))
    files["etc/init.d/S00init"] = (s00, before["etc/init.d/S00init"]["mode"])
    updater = inputs / "camera-update"
    updater.write_bytes(camera_update_script(contents(pseudo, "bin/camera-update")))
    files["bin/camera-update"] = (updater, before["bin/camera-update"]["mode"])
    sums = inputs / "cam.sha256"
    sums.write_text(f"{cam_info['sha256']}  /mnt/usr/bin/cam\n")
    files["etc/mi360/cam.sha256"] = (sums, "600")
    replaced, retained = retained_scope(before, removed, files)
    filtered = [line for line in lines if not line or line.startswith("#") or line.split()[0] not in removed | replaced]
    recipe = out / "retained.pseudo"
    recipe.write_bytes(("\n".join(filtered) + "\n").encode() + MARKER + data)
    overlay = [f"etc/mi360 D {TIMESTAMP} 700 0 0", f"etc/mi360/network D {TIMESTAMP} 700 0 0"]
    for name, (path, mode) in sorted(files.items()):
        uid, gid = (before[name]["uid"], before[name]["gid"]) if name in replaced else (0, 0)
        overlay.append(f"{name} F {TIMESTAMP} {mode} {uid} {gid} cat {shlex.quote(str(path))}")
    overlay_path = out / "overlay.pseudo"
    overlay_path.write_text("\n".join(overlay) + "\n")
    empty = out / "empty"
    empty.mkdir()
    image = out / "usrfs-mi360.squashfs"
    root = before["/"]
    log = run("mksquashfs", str(empty), str(image), "-pf", str(recipe), "-pf", str(overlay_path),
              "-comp", "xz", "-b", "131072", "-noappend", "-no-xattrs", "-no-progress",
              "-processors", "2", "-mem", "256M", "-mkfs-time", str(TIMESTAMP),
              "-root-time", str(root["mtime"]), "-root-mode", root["mode"],
              "-root-uid", str(root["uid"]), "-root-gid", str(root["gid"]))
    (out / "mksquashfs.log").write_bytes(log)
    candidate = out / "candidate.pseudo"
    run("unsquashfs", "-no-progress", "-pf", str(candidate), str(image))
    after, _, _ = pseudo_records(candidate.read_bytes())
    added = files.keys() - before.keys() | {"etc/mi360", "etc/mi360/network"}
    if after.keys() - before.keys() != added or before.keys() - after.keys() != removed:
        raise ValueError("unexpected filesystem path additions/removals")
    for name in retained:
        if after[name] != before[name]:
            raise ValueError(f"retained file/content/metadata changed: {name}")
    for name, (path, mode) in files.items():
        expected = {"type": "R", "mtime": TIMESTAMP, "mode": mode,
                    "uid": before[name]["uid"] if name in replaced else 0,
                    "gid": before[name]["gid"] if name in replaced else 0,
                    "size": path.stat().st_size, "sha256": sha(path.read_bytes())}
        if after[name] != expected:
            raise ValueError(f"packaged source mismatch: {name}")
    payload = image.read_bytes()
    if payload[:4] != b"hsqs" or struct.unpack_from("<IH", payload, 12)[0] != 131072 or struct.unpack_from("<H", payload, 20)[0] != 4:
        raise ValueError("unexpected SquashFS compressor/block size")
    padded, programmer = patch_flash(base, payload)
    (out / "B_USRFS.bin").write_bytes(padded)
    (out / "camera-mi360-16MiB.bin").write_bytes(programmer)
    partitions = {}
    for name, start, end in PARTITIONS:
        partitions[name] = {"offset": start, "size": end - start,
                            "base_sha256": sha(base[start:end]), "output_sha256": sha(programmer[start:end]),
                            "identical": base[start:end] == programmer[start:end]}
        if name != "B_USRFS" and not partitions[name]["identical"]:
            raise ValueError(f"protected partition changed: {name}")
    report = {"schema": 1, "base_sha256": sha(base), "bootstrap": bootstrap_report, "image_sha256": sha(programmer),
              "image_bytes": len(programmer), "changed_partition": "B_USRFS",
              "changed_interval": [START, START + SIZE], "protected_bytes_identical": True,
              "cam": cam_info, "mtd_writer": writer_info, "init_whitelist": sorted(KEEP_INIT | {"S99mi360_network", "S99zzz_mi360"}),
              "removed_paths": sorted(removed), "added_paths": sorted(added), "replaced_paths": sorted(replaced),
              "retained_paths_checked": len(retained),
              "all_retained_contents_and_metadata_identical": True,
              "retained_manifest_sha256": sha(json.dumps({k: before[k] for k in sorted(retained)}, sort_keys=True).encode()),
              "packaged_files": {k: after[k] for k in sorted(files)}, "partitions": partitions,
              "tools": versions, "fixed_new_file_timestamp": TIMESTAMP,
              "builder_sha256": sha(Path(__file__).read_bytes()),
              "camera_update_base_sha256": CAMERA_UPDATE_SHA256,
              "usrfs_sha256": sha(payload), "padded_usrfs_sha256": sha(padded),
              "usrfs_file_bytes": len(payload), "usrfs_bytes_used": struct.unpack_from("<Q", payload, 40)[0],
              "partition_bytes": SIZE, "spare_bytes": SIZE - len(payload),
              "hardware_boot_verified": False, "hardware_flash_verified": False,
              "publication_or_installation_performed": False}
    (out / "manifest.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({k: report[k] for k in ("image_sha256", "image_bytes", "usrfs_file_bytes", "spare_bytes", "protected_bytes_identical")}, indent=2))


if __name__ == "__main__":
    main()
