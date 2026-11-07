#!/usr/bin/env python3
"""Isolated host policy tests; no camera, flash, SDK, network or credentials."""
import importlib.util
from pathlib import Path
import re
import struct
import subprocess
import tempfile
import time
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("firmware_build", HERE / "build_image.py")
build = importlib.util.module_from_spec(spec)
spec.loader.exec_module(build)


def sandbox_paths(source, root):
    replacements = {"/mnt/usr": str(root / "usr"), "/mnt/data": str(root / "data"),
                    "/tmp/mi360": str(root / "run/mi360")}
    # One pass: a Linux temporary root can itself start with /tmp/mi360.
    return re.sub(r"/mnt/usr|/mnt/data|/tmp/mi360", lambda match: replacements[match[0]], source)


def elf_fixture():
    value = bytearray(104)
    value[:7] = b"\x7fELF\x01\x01\x01"
    struct.pack_into("<HHI", value, 16, 2, 8, 1)
    struct.pack_into("<I", value, 28, 52)
    struct.pack_into("<I", value, 36, 0x70001007)
    struct.pack_into("<HH", value, 42, 32, 1)
    struct.pack_into("<8I", value, 52, 3, 84, 0, 0, 20, 20, 4, 1)
    value[84:] = b"/lib/ld-uClibc.so.0\0"
    return value


class ImagePolicy(unittest.TestCase):
    def test_updater_patch_requires_exact_hash_and_one_original_block(self):
        with self.assertRaisesRegex(ValueError, "hash differs"):
            build.camera_update_script(b"unreviewed-updater")
        prefix = b"#!/bin/sh\nset -eu\n"
        suffix = (b"state=/tmp/camera-update.staged\n"
                  b'        camera-mtd-write check kernel "$2"\n'
                  b'        camera-mtd-write check usrfs "$3"\n'
                  b'        camera-mtd-write write kernel "$2"\n'
                  b'        camera-mtd-write write usrfs "$3"\n')
        original = prefix + build.OTA_QUIESCE_ORIGINAL + suffix
        # A synthetic carrier tests the hash and anchor guards independently;
        # no fixture, process or byte is sent to a camera.
        with patch.object(build, "CAMERA_UPDATE_SHA256", build.sha(original)):
            actual = build.camera_update_script(original)
        expected_suffix = suffix.replace(b"camera-mtd-write check", b"/mnt/usr/bin/camera-mtd-write plan").replace(
            b"camera-mtd-write write", b"/mnt/usr/bin/camera-mtd-write write")
        self.assertEqual(actual, prefix + build.OTA_QUIESCE_REPLACEMENT + expected_suffix)
        with patch.object(build, "CAMERA_UPDATE_SHA256", build.sha(original)):
            ram = build.camera_update_script(original,"/tmp/mi360-update-tools/camera-mtd-write")
            for bad in ("camera-mtd-write", "/tmp/camera-mtd-write", "/tmp/../camera-mtd-write", "/tmp/tool;bad/camera-mtd-write"):
                with self.assertRaisesRegex(ValueError,"writer path"):
                    build.camera_update_script(original,bad)
        self.assertNotIn(b"        camera-mtd-write ",ram)
        self.assertEqual(ram.count(b"/tmp/mi360-update-tools/camera-mtd-write "),4)
        self.assertLess(ram.index(b" plan usrfs "),ram.index(b" write kernel "))
        for invalid in (prefix + suffix, original + build.OTA_QUIESCE_ORIGINAL):
            with patch.object(build, "CAMERA_UPDATE_SHA256", build.sha(invalid)):
                with self.assertRaisesRegex(ValueError, "block differs"):
                    build.camera_update_script(invalid)

    def test_updater_missing_preflight_or_write_anchor_is_refused(self):
        source = (b"#!/bin/sh\n" + build.OTA_QUIESCE_ORIGINAL +
                  b'        camera-mtd-write check kernel "$2"\n'
                  b'        camera-mtd-write check usrfs "$3"\n'
                  b'        camera-mtd-write write kernel "$2"\n')
        with patch.object(build,"CAMERA_UPDATE_SHA256",build.sha(source)):
            with self.assertRaisesRegex(ValueError,"writer call differs"):
                build.camera_update_script(source)

    def test_exact_only_application_partition_is_replaced(self):
        # Synthetic bytes are private in-memory fixtures, never device input.
        base = bytes(range(256)) * (build.FLASH_SIZE // 256)
        padded, result = build.patch_flash(base, b"private-test-squashfs")
        self.assertEqual(len(result), 0x1000000)
        self.assertEqual(len(padded), 0x470000)
        for name, start, end in build.PARTITIONS:
            if name != "B_USRFS":
                self.assertEqual(result[start:end], base[start:end], name)
        self.assertEqual(padded[21:], b"\xff" * (build.SIZE - 21))

    def test_exact_four_replacements_preserve_the_other_stock_paths(self):
        before = {f"retained/{index}": {} for index in range(build.RETAINED_STOCK_COUNT)}
        before.update({name:{} for name in build.REPLACED_STOCK})
        before["bin/fetch_av"] = {}
        files = {name:None for name in build.REPLACED_STOCK}
        files["bin/cam"] = None
        replaced, retained = build.retained_scope(before,{"bin/fetch_av"},files)
        self.assertEqual(replaced,{"etc/init.d/S00init","etc/init.d/S60service","bin/camera-update","bin/camera-mtd-write"})
        self.assertEqual(len(retained),build.RETAINED_STOCK_COUNT)
        with self.assertRaisesRegex(ValueError,"four stock replacements"):
            build.retained_scope(before,{"bin/fetch_av"},dict(files, **{"retained/0":None}))
        before.pop("retained/0")
        with self.assertRaisesRegex(ValueError,f"{build.RETAINED_STOCK_COUNT} retained"):
            build.retained_scope(before,{"bin/fetch_av"},files)

    def test_wrong_flash_size_and_partition_overflow_fail(self):
        for base, payload in ((b"short", b"x"), (b"x" * build.FLASH_SIZE, b"x" * (build.SIZE + 1))):
            with self.assertRaises(ValueError):
                build.patch_flash(base, payload)

    def test_elf_rejects_wrong_cpu_abi_loader_and_truncation(self):
        good = elf_fixture()
        self.assertEqual(build.check_cam(good)["interpreter"], "/lib/ld-uClibc.so.0")
        cases = [good[:40]]
        for offset, value in ((4, 2), (5, 2), (16, 3), (18, 40), (39, 0x50), (44, 65), (84, ord("x"))):
            bad = good.copy()
            bad[offset] = value
            cases.append(bad)
        for value in cases:
            with self.assertRaises(ValueError):
                build.check_cam(value)

    def test_pseudo_manifest_hashes_original_bytes_and_metadata(self):
        pseudo = b"/ D 123 755 1000 1000\nlib D 123 755 1000 1000\nlib/a R 123 555 1000 1000 3 0 0\nlib/link S 124 777 0 0 a\n" + build.MARKER + b"abc"
        manifest, _, _ = build.pseudo_records(pseudo)
        self.assertEqual(manifest["lib/a"]["sha256"], build.sha(b"abc"))
        self.assertEqual(manifest["lib/a"]["uid"], 1000)
        self.assertEqual(manifest["lib/link"]["target"], "a")
        self.assertEqual(build.contents(pseudo, "lib/a"), b"abc")
        for bad in (pseudo.replace(b"3 0 0", b"4 0 0"), pseudo.replace(b"lib/a R", b"../a R"), pseudo.replace(b"3 0 0", b"3 0 1")):
            with self.assertRaises(ValueError):
                build.pseudo_records(bad)

    def test_s00_edit_is_strict_and_preserves_original_day_pulse(self):
        original = b"    telnetd &\n\t#set ircult to day\noriginal-day-pulse\n"
        actual = build.init_script(original)
        self.assertNotIn(b"telnetd &", actual)
        self.assertTrue(actual.endswith(b"\t#set ircult to day\noriginal-day-pulse\n"))
        self.assertIn(b"for pin in 9 49 57", actual)
        self.assertIn(b"echo low", actual)
        for bad in (original.replace(b"telnetd", b"other"), original + original):
            with self.assertRaises(ValueError):
                build.init_script(bad)

    def test_removal_is_explicit_and_never_media_libraries_tuning_or_recovery(self):
        base = {"bin/" + name: {} for name in build.REMOVE_BINS}
        base.update({"lib/" + name: {} for name in build.REMOVE_LIBS})
        kept = ("lib/libimp.so", "lib/libalog.so", "lib/libaudioProcess.so", "lib/libnl-tiny.so", "lib/libcurl.so.4.4.0",
                "ko/sensor_sc2336_t31.ko", "etc/sensor/sc2336.bin", "etc/init.d/S99zz_rootssh", "etc/init.d/S15watchdog")
        for name in kept + ("etc/init.d/S99netcheck",):
            base[name] = {}
        removed = build.removed_paths(base)
        self.assertIn("etc/init.d/S99netcheck", removed)
        reviewed = {"bin/" + name for name in build.REMOVE_BINS} | {"lib/" + name for name in build.REMOVE_LIBS}
        self.assertEqual(removed - reviewed, {"etc/init.d/S99netcheck"})
        self.assertFalse(removed & set(kept))
        for missing in ("bin/fetch_av", "lib/libpersonDet_inf.so"):
            partial = dict(base)
            partial.pop(missing)
            with self.assertRaises(ValueError):
                build.removed_paths(partial)


class UpdaterPolicy(unittest.TestCase):
    def quiesce(self, supervisor_status, live_checks):
        # Execute the production replacement function. Only its filesystem root
        # and external process/supervisor/wait edges use private host fixtures.
        with tempfile.TemporaryDirectory(prefix="mi360-updater-test-") as directory:
            root = Path(directory)
            (root / "perp/miio_ota").mkdir(parents=True)
            function = build.OTA_QUIESCE_REPLACEMENT.decode().replace("/tmp/perp", str(root / "perp"))
            script = """
set -eu
supervisor_status=$1
live_checks=$2
events=$3
checks=0
fail() { printf '%s\\n' "$*" >&2; exit 1; }
perpctl() { printf 'perp:%s\\n' "$*" >>"$events"; return "$supervisor_status"; }
pidof() {
    checks=$((checks + 1))
    printf 'pid:%s\\n' "$*" >>"$events"
    [ "$checks" -le "$live_checks" ]
}
sleep() { printf 'wait:%s\\n' "$*" >>"$events"; }
""" + function + "\nquiesce_ota\nprintf 'quiesced\\n'\n"
            result = subprocess.run(["/bin/sh", "-s", "--", str(supervisor_status), str(live_checks), str(root / "events")],
                                    input=script, capture_output=True, text=True, timeout=3)
            return result, (root / "events").read_text().splitlines()

    def test_inactive_supervisor_metadata_without_ota_process_is_safe(self):
        for code in (0, 111):
            result, events = self.quiesce(code, 0)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout, "quiesced\n")
            self.assertEqual(events, ["perp:d miio_ota", "pid:miio_ota"])

    def test_actual_ota_process_prevents_update_after_bounded_wait(self):
        for code in (0, 111):
            result, events = self.quiesce(code, 5)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("Vendor OTA is still running", result.stderr)
            self.assertNotIn("quiesced", result.stdout)
            self.assertEqual(events.count("pid:miio_ota"), 5)
            self.assertEqual(events.count("wait:1"), 5)

    def test_exiting_ota_is_checked_again_before_update(self):
        result, events = self.quiesce(111, 2)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(events.count("pid:miio_ota"), 3)
        self.assertEqual(events.count("wait:1"), 2)


class BootPolicy(unittest.TestCase):
    def shell(self, program):
        return subprocess.run(["/bin/sh", "-c", '. "$1"\n' + program, "policy-test", str(HERE / "boot-policy.sh")], capture_output=True, text=True, timeout=3)

    def test_attempt_budget_refuses_corruption_negative_values_and_fourth_boot(self):
        result = self.shell("""
for value in 0 1 2; do attempt_next "$value" || exit 1; done
for value in '' 3 4 -1 01 000 x 999999999999; do
    if attempt_next "$value"; then exit 2; fi
done
""")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "1\n2\n3\n")

    def test_health_requires_fresh_identity_sequence_and_every_enabled_media_counter(self):
        result = self.shell("""
health_qualified 42 42 realboot realboot true 2 1 30 20 30 20 100 80 true || exit 1
health_qualified 42 42 realboot realboot true 2 1 30 20 30 20 0 0 false || exit 2
if health_qualified 41 42 realboot realboot true 2 1 30 20 30 20 100 80 true; then exit 3; fi
if health_qualified 42 42 stale realboot true 2 1 30 20 30 20 100 80 true; then exit 4; fi
if health_qualified 42 42 realboot realboot false 2 1 30 20 30 20 100 80 true; then exit 5; fi
if health_qualified 42 42 realboot realboot true 1 1 30 20 30 20 100 80 true; then exit 6; fi
if health_qualified 42 42 realboot realboot true 2 1 20 20 30 20 100 80 true; then exit 7; fi
if health_qualified 42 42 realboot realboot true 2 1 30 20 30 20 80 80 true; then exit 8; fi
if health_qualified 42 42 realboot realboot true 2 1 30 40 30 20 100 80 true; then exit 9; fi
""")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_large_media_counts_do_not_overflow_mips_shell_arithmetic(self):
        result = self.shell("""
decimal_greater 4294967296 4294967295 || exit 1
decimal_greater 10000000000000 9999999999999 || exit 2
if decimal_greater invalid 42; then exit 3; fi
if decimal_greater '' 42; then exit 4; fi
if decimal_greater 42 43; then exit 5; fi
""")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_scripts_parse_without_execution(self):
        for name in ("boot-policy.sh", "launcher.sh", "network-start.sh", "S60service", "S99mi360_network", "S99zzz_mi360"):
            result = subprocess.run(["/bin/sh", "-n", str(HERE / name)], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)

    def test_health_deadlines_use_startup_grace_then_bounded_staleness(self):
        result = self.shell("""
if health_expired 0 190 100 100; then exit 1; fi
health_expired 0 191 100 100 || exit 2
if health_expired 1 215 100 200; then exit 3; fi
health_expired 1 216 100 200 || exit 4
""")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.splitlines(), ["startup health deadline exceeded", "qualified health stopped advancing"])

    def test_failed_data_preparation_starts_packaged_ap_without_touching_data(self):
        with tempfile.TemporaryDirectory(prefix="mi360-network-test-") as directory:
            root = Path(directory)
            (root / "data").mkdir()
            (root / "run").mkdir()
            actual = root / "data/original"
            actual.mkdir()
            (actual / "wpa.conf").write_text("isolated-private-network-fixture")
            (root / "data/mi360-network").symlink_to(actual, target_is_directory=True)
            package = root / "usr/etc/mi360/network"
            package.mkdir(parents=True)
            marker = root / "ap-started"
            (package / "ap.sh").write_text("#!/bin/sh\nprintf started >'" + str(marker) + "'\n")
            source = sandbox_paths((HERE / "network-start.sh").read_text(), root)
            script = root / "network-start.sh"
            script.write_text(source)
            result = subprocess.run(["/bin/sh", str(script)], capture_output=True, text=True, timeout=3)
            self.assertNotEqual(result.returncode, 0)
            until = time.monotonic() + 1
            while not marker.exists() and time.monotonic() < until:
                time.sleep(0.01)
            self.assertTrue(marker.exists())
            self.assertEqual((actual / "wpa.conf").read_text(), "isolated-private-network-fixture")

    def test_attempt_commit_propagates_each_failure_even_in_conditional_context(self):
        source = (HERE / "launcher.sh").read_text()
        function = source.split("write_attempts() {", 1)[1].split("\n}", 1)[0]
        function = "write_attempts() {" + function + "\n}\n"
        edges = ["rm", "printf", "chmod", "chown", "sync1", "mv", "sync2"]
        for failure in ["none", *edges]:
            with self.subTest(failure=failure), tempfile.TemporaryDirectory(prefix="mi360-counter-test-") as directory:
                root = Path(directory)
                (root / "attempts").write_text("1\n")
                events = root / "events"
                script = r"""
set -eu
base=$1
failure=$2
events=$base/events
sync_count=0
edge() { command printf '%s\n' "$1" >>"$events"; [ "$failure" != "$1" ]; }
rm() { edge rm || return 1; command rm "$@"; }
printf() { edge printf || return 1; command printf "$@"; }
chmod() { edge chmod || return 1; command chmod "$@"; }
chown() { edge chown; }
sync() { sync_count=$((sync_count + 1)); edge "sync$sync_count"; }
mv() { edge mv || return 1; command mv "$@"; }
""" + function + r"""
# This is the production caller's conditional context, where set -e alone
# cannot make the function abort at the first failing command.
if write_attempts 2; then exit 0; else exit 7; fi
"""
                result = subprocess.run(["/bin/sh", "-s", "--", str(root), failure], input=script,
                                        capture_output=True, text=True, timeout=3)
                expected = edges if failure == "none" else edges[:edges.index(failure) + 1]
                self.assertEqual(events.read_text().splitlines(), expected, result.stderr)
                self.assertEqual(result.returncode, 0 if failure == "none" else 7, result.stderr)
                # The final sync can fail after rename; that is uncertainty,
                # never success. Earlier failures must preserve the old counter.
                committed = failure in ("none", "sync2")
                self.assertEqual((root / "attempts").read_text(), "2\n" if committed else "1\n")

    def test_disabled_and_exhausted_launches_do_not_call_binary_or_watchdog(self):
        # Execute production early paths with filesystem roots redirected to a
        # private temporary directory. Nothing reaches camera/system paths.
        for enabled, attempts in (("0", "0"), ("1", "3"), ("1", "bad")):
            with tempfile.TemporaryDirectory(prefix="mi360-boot-test-") as directory:
                root = Path(directory)
                base = root / "data/mi360"
                base.mkdir(parents=True, mode=0o700)
                for name, value in (("enabled", enabled), ("attempts", attempts), ("cam.toml", "isolated-private-fixture")):
                    path = base / name
                    path.write_text(value + "\n")
                    path.chmod(0o600)
                policy = root / "usr/etc/mi360"
                policy.mkdir(parents=True)
                (policy / "boot-policy.sh").write_bytes((HERE / "boot-policy.sh").read_bytes())
                (root / "run").mkdir()
                source = sandbox_paths((HERE / "launcher.sh").read_text(), root)
                # Root ownership is production policy; tests use their own UID.
                source = source.replace('[ "$3" = 0 ]', '[ "$3" = "$(id -u)" ]')
                script = root / "launcher.sh"
                script.write_text(source)
                result = subprocess.run(["/bin/sh", str(script)], capture_output=True, text=True, timeout=3)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn("recovery-only", result.stdout)
                self.assertFalse((base / "attempts.new").exists())


if __name__ == "__main__":
    unittest.main()
