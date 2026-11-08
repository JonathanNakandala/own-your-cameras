#!/usr/bin/env python3
"""Install the native application on an untouched DS-KD8003-IME1 (V2.2.62 build 230204).

Everything not given as a flag or in --config is asked on the terminal; without a terminal the
installer stops and names what is missing. It talks only to the panel you name:

  1. checks the model and firmware over ISAPI and enables the stock SSH service;
  2. reads the firmware partition over stock SSH, rebuilds the stock package from it, checks it by
     hash, takes the keypad key from it, adds the startup hook and flashes the result over ISAPI
     (skipped when the hook is already installed);
  3. uploads the application, the HiSilicon libraries, the configuration and your SSH key;
  4. switches the panel to the native application and checks that video, MQTT and the keypad start.

Roll back by deleting /home/config/custom/native.enabled and rebooting; two unhealthy boots in a row
return to the vendor application by themselves.
"""
import argparse
import getpass
import hashlib
import io
import json
import lzma
import math
import os
import re
import struct
import sys
import tarfile
import tempfile
import time
import tomllib
import urllib.error
import urllib.request
import xml.etree.ElementTree as ET
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

MODEL = "DS-KD8003-IME1"
FIRMWARE = ("V2.2.62", "build 230204")
MARKER = "ds-kd8003-ime1-custom-boot-v1"
BASE = "/home/config/custom"
RUNTIME = "/home/app/custom/native"
FIRMWARE_PARTITION = ("mtd1", 0xE20000)
CONFIG_PARTITION_FREE_MIN = 64 * 1024
# Payload files and where they come from in the release bundle.
PAYLOAD = ("ds-kd8003-ime1-cam", "libloader_compat.so", "libnative_compat.so", "libnative_media_group.so")
THIRD_PARTY = ("libmpi.so", "libVoiceEngine.so", "libdnvqe.so", "libupvqe.so")

# What may be set: (key, question, default, secret). None as default means required.
SETTINGS = (
    ("host", "Panel address (IP or host name)", None, False),
    ("admin_password", "Panel administrator password", None, True),
    ("mqtt_host", "MQTT broker address", None, False),
    ("mqtt_port", "MQTT broker port", "1883", False),
    ("mqtt_user", "MQTT user (empty for none)", "", False),
    ("mqtt_password", "MQTT password (empty for none)", "", True),
    ("device_name", "Name in Home Assistant", MODEL, False),
    ("rtsp_user", "RTSP user for recorders and Home Assistant", "admin", False),
    ("rtsp_password", "RTSP password", None, True),
    ("pins", "PIN table file (<digits> <name> [clip] per line)", None, False),
    ("ssh_key", "SSH public key that may log in after the switch", "~/.ssh/id_ed25519.pub", False),
    ("clips", "Directory with your own <name>.pcm clips (empty for built-in tones)", "", False),
)
# Optional cam.conf keys that are never asked: set them with a flag or in --config.
ADVANCED = ("mqtt_prefix", "mqtt_node", "http_port")
# Live settings in Home Assistant now (settings.json on the panel, kept across installs). Given in
# --config or as flags, they go into cam.conf so a first start without settings.json seeds them.
DEPRECATED = ("open_seconds", "talk_db", "clip_db", "mic_gain_db", "codec_out_db")
SECRET_ENV = {"admin_password": "DS_KD_ADMIN_PASSWORD", "mqtt_password": "DS_KD_MQTT_PASSWORD",
              "rtsp_password": "DS_KD_RTSP_PASSWORD"}


def say(text):
    print(f"==> {text}", flush=True)


def settings(args):
    """Flags win over the config file, which wins over environment secrets; the rest is asked."""
    values = {}
    if args.config:
        values.update({k: str(v) for k, v in tomllib.loads(Path(args.config).read_text()).items()})
    for key, env in SECRET_ENV.items():
        if key not in values and os.environ.get(env):
            values[key] = os.environ[env]
    known = {s[0] for s in SETTINGS} | set(ADVANCED) | set(DEPRECATED)
    values.update({k: v for k, v in vars(args).items() if k in known and v is not None})
    for key in sorted(set(values) & set(DEPRECATED)):
        say(f"{key} is a live setting in Home Assistant now; it only seeds a panel without settings.json")
    unknown = set(values) - known
    if unknown:
        raise SystemExit(f"Unknown settings: {', '.join(sorted(unknown))}")
    missing = [s for s in SETTINGS if s[0] not in values]
    if missing and not sys.stdin.isatty():
        required = [s[0] for s in missing if s[2] is None]
        if required:
            raise SystemExit(f"Not set and no terminal to ask: {', '.join(required)}")
    for key, question, default, secret in missing:
        if not sys.stdin.isatty():
            values[key] = default
            continue
        while True:
            hint = f" [{default}]" if default else ""
            answer = getpass.getpass(f"{question}: ") if secret else input(f"{question}{hint}: ").strip()
            answer = answer or (default if default is not None else "")
            if answer or default is not None:
                break
        values[key] = answer
    return values


def cam_conf(v):
    for key in ("device_name", "mqtt_user", "mqtt_password", "rtsp_user", "rtsp_password"):
        if re.search(r'[\n\r"\\]', v[key]):
            raise SystemExit(f"{key} may not contain quotes, backslashes or line breaks")
    lines = [
        f"mqtt_host={v['mqtt_host']}", f"mqtt_port={int(v['mqtt_port'])}", f"mqtt_user={v['mqtt_user']}",
        f"mqtt_password={v['mqtt_password']}", f"device_name={v['device_name']}",
        f"rtsp_user={v['rtsp_user']}", f"rtsp_password={v['rtsp_password']}",
    ]
    for key in ADVANCED + DEPRECATED:
        if v.get(key) not in (None, ""):
            if not re.fullmatch(r"[A-Za-z0-9_/-]+", str(v[key])):
                raise SystemExit(f"{key} has characters cam.conf does not accept")
            lines.append(f"{key}={v[key]}")
    return ("\n".join(lines) + "\n").encode()


def carried_settings(old_conf, v):
    """Live-setting keys of the panel's current cam.conf that this run does not set: an upgrade from a
    release without settings.json keeps a custom open_seconds or gain by seeding it once."""
    carried = {}
    for line in old_conf.splitlines():
        key, sep, value = line.strip().partition("=")
        if sep and key in DEPRECATED and v.get(key) in (None, "") and re.fullmatch(r"-?[0-9]+", value.strip()):
            carried[key] = value.strip()
    return carried


def pins_conf(path):
    text = Path(path).expanduser().read_text()
    codes = []
    for n, line in enumerate(text.splitlines(), 1):
        line = line.split("#", 1)[0].strip()
        if not line:
            continue
        parts = line.split()
        if not (2 <= len(parts) <= 3 and re.fullmatch(r"\d{4,8}", parts[0])
                and all(re.fullmatch(r"[A-Za-z0-9_-]+", p) for p in parts[1:])):
            raise SystemExit(f"{path}:{n}: expected <4-8 digits> <name> [clip]")
        codes.append(parts[0])
    for a in codes:
        for b in codes:
            if a != b and a in b:
                raise SystemExit(f"{path}: one code contains another, so the longer could never open")
    if not codes:
        raise SystemExit(f"{path}: no codes")
    return text.encode(), len(codes)


def tone_clip(*notes, rate=16000):
    """A short chime as raw s16le mono 16 kHz: (frequency, seconds) per note, soft attack and decay."""
    out = bytearray()
    for freq, seconds in notes:
        n = int(rate * seconds)
        for i in range(n):
            envelope = min(1.0, i / (0.01 * rate)) * (1 - i / n) ** 2
            out += struct.pack("<h", int(12000 * envelope * math.sin(2 * math.pi * freq * i / rate)))
    return bytes(out)


def residual_encode(pcm):
    """Raw s16le mono as second-order prediction residuals, wrapping in 16 bits: lossless.

    The daemon restores the exact samples (src/clip.rs). The residuals compress about a fifth better
    under LZMA, which matters on the small config partition."""
    out, p1, p2 = bytearray(), 0, 0
    for (x,) in struct.iter_unpack("<h", pcm[: len(pcm) // 2 * 2]):
        r = (x - 2 * p1 + p2 + 32768) % 65536 - 32768
        out += struct.pack("<h", r)
        p1, p2 = x, p1
    return bytes(out)


def partition_free(shell):
    """Bytes free on the config partition."""
    return int([l for l in shell.check(f"df -k {BASE}").splitlines() if l.strip()][-1].split()[3]) * 1024


def existing_size(shell, path):
    """Size of a file on the panel; 0 when it is absent."""
    status, out = shell.run(f"/bin/busybox wc -c < {path} 2>/dev/null")
    words = out.split()
    return int(words[0]) if status == 0 and words and words[0].isdigit() else 0


def clips_tar(directory):
    pcm = {"door_opened": tone_clip((660, 0.18), (880, 0.35))}
    if directory:
        for path in sorted(Path(directory).expanduser().glob("*.pcm")):
            pcm[path.stem] = path.read_bytes()
    clips = {f"{name}.pcmd": residual_encode(data) for name, data in pcm.items()}
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w", format=tarfile.USTAR_FORMAT) as tar:
        for name, data in clips.items():
            info = tarfile.TarInfo(name)
            info.size, info.mode, info.mtime = len(data), 0o600, 0
            tar.addfile(info, io.BytesIO(data))
    return buffer.getvalue()


class Isapi:
    def __init__(self, host, password):
        self.base = f"http://{host}"
        self.password = password
        manager = urllib.request.HTTPPasswordMgrWithDefaultRealm()
        manager.add_password(None, self.base, "admin", password)
        self.opener = urllib.request.build_opener(urllib.request.HTTPDigestAuthHandler(manager))

    def request(self, path, data=None, method=None, timeout=20, content_type="application/xml"):
        req = urllib.request.Request(self.base + path, data=data, method=method)
        if data is not None:
            req.add_header("Content-Type", content_type)
        with self.opener.open(req, timeout=timeout) as response:
            return response.read()

    def device(self):
        root = ET.fromstring(self.request("/ISAPI/System/deviceInfo"))
        return {el.tag.split("}")[-1]: (el.text or "") for el in root}

    def ssh_enabled(self):
        return b"<enabled>true</enabled>" in self.request("/ISAPI/System/Network/ssh").replace(b" ", b"")

    def enable_ssh(self):
        self.request("/ISAPI/System/Network/ssh", b"<SSH><enabled>true</enabled></SSH>", "PUT")

    def flash(self, dav):
        """PUT the package with a precomputed Digest header: sent without one, the panel answers 401 and
        closes the connection before a large body is through."""
        import http.client
        path = "/ISAPI/System/updateFirmware"
        host = self.base.removeprefix("http://")
        connection = http.client.HTTPConnection(host, timeout=30)
        connection.request("PUT", path, body=b"", headers={"Content-Type": "application/octet-stream"})
        challenge = connection.getresponse()
        challenge.read()
        header = challenge.getheader("WWW-Authenticate", "")
        connection.close()
        fields = dict(re.findall(r'(\w+)="?([^",]*)"?', header.removeprefix("Digest ")))
        if challenge.status != 401 or "nonce" not in fields:
            raise IOError(f"the panel did not ask for Digest authentication ({challenge.status})")
        md5 = lambda text: hashlib.md5(text.encode()).hexdigest()
        ha1 = md5(f"admin:{fields['realm']}:{self.password}")
        ha2 = md5(f"PUT:{path}")
        cnonce, nc = os.urandom(8).hex(), "00000001"
        if "auth" in fields.get("qop", "").split(","):
            response = md5(f"{ha1}:{fields['nonce']}:{nc}:{cnonce}:auth:{ha2}")
            extra = f', qop=auth, nc={nc}, cnonce="{cnonce}"'
        else:
            response, extra = md5(f"{ha1}:{fields['nonce']}:{ha2}"), ""
        opaque = f', opaque="{fields["opaque"]}"' if "opaque" in fields else ""
        auth = (f'Digest username="admin", realm="{fields["realm"]}", nonce="{fields["nonce"]}", uri="{path}", '
                f'response="{response}"{extra}{opaque}')
        connection = http.client.HTTPConnection(host, timeout=600)
        connection.request("PUT", path, body=dav, headers={"Content-Type": "application/octet-stream", "Authorization": auth})
        result = connection.getresponse()
        text = result.read().decode("utf-8", "replace")
        connection.close()
        if result.status != 200:
            raise IOError(f"the panel refused the firmware ({result.status}): {text[:300]}")


class Shell:
    """Root shell on the stock SSH service (password, no terminal) or on our key-only Dropbear."""
    START = b"__DS_KD_START__"
    DONE = b"__DS_KD_DONE__"

    def __init__(self, host, password=None, key=None, known_hosts=None):
        import paramiko
        self.client = paramiko.SSHClient()
        if known_hosts and known_hosts.exists():
            self.client.load_host_keys(str(known_hosts))
        self.client.set_missing_host_key_policy(paramiko.AutoAddPolicy())
        self.client.connect(host, username="root", password=password, key_filename=key, timeout=15,
                            allow_agent=False, look_for_keys=False, banner_timeout=30)
        if known_hosts:
            self.client.save_host_keys(str(known_hosts))

    def channel(self):
        ch = self.client.get_transport().open_session(timeout=15)
        ch.set_combine_stderr(True)
        ch.settimeout(30)
        ch.invoke_shell()  # no terminal: the stock profile's restricted shell exits, /bin/sh reads on
        return ch

    def run(self, script, timeout=120):
        ch = self.channel()
        try:
            # The stock shell prints a banner first; only what follows START belongs to the script. The script's
            # status is saved before the separator echo, which would otherwise be the `$?` reported.
            ch.sendall(b"echo " + self.START + b"\n" + script.encode() + b"\nrc=$?; echo; echo " + self.DONE + b" $rc\nexit\n")
            out, end = b"", time.monotonic() + timeout
            while not re.search(self.DONE + rb" \d+\n", out):
                if time.monotonic() > end:
                    raise TimeoutError(f"no answer from the panel for: {script[:60]}")
                chunk = ch.recv(65536)
                if not chunk:
                    break
                out += chunk
            m = re.search(self.DONE + rb" (\d+)\n", out)
            if not m:
                raise IOError(f"the panel closed the session during: {script[:60]}")
            body = out[: m.start()]
            begin = body.find(self.START + b"\n")
            if begin >= 0:
                body = body[begin + len(self.START) + 1 :]
            return int(m[1]), body.decode("utf-8", "replace")
        finally:
            ch.close()

    def check(self, script, timeout=120):
        status, out = self.run(script, timeout)
        if status:
            raise IOError(f"panel command failed ({status}): {script[:60]}\n{out[-400:]}")
        return out

    def read(self, path, size):
        ch = self.channel()
        try:
            ch.sendall(f"printf 'BEGIN_DATA\\n'; exec /bin/busybox cat {path}\n".encode())
            buf = b""
            while b"BEGIN_DATA\n" not in buf:
                chunk = ch.recv(65536)
                if not chunk:
                    raise IOError(f"could not read {path}")
                buf += chunk
            data = bytearray(buf.split(b"BEGIN_DATA\n", 1)[1])
            ch.settimeout(60)
            while len(data) < size:
                chunk = ch.recv(1 << 20)
                if not chunk:
                    break
                data += chunk
            if len(data) != size:
                raise IOError(f"read {len(data)} of {size} bytes from {path}")
            return bytes(data)
        finally:
            ch.close()

    def write(self, path, data, mode="600"):
        ch = self.channel()
        try:
            ch.sendall(f"umask 077; exec 3>{path}.part; printf 'READY_DATA\\n'; exec /bin/busybox cat >&3\n".encode())
            buf, end = b"", time.monotonic() + 30
            while b"READY_DATA\n" not in buf:
                if time.monotonic() > end:
                    raise TimeoutError(f"panel not ready to receive {path}")
                buf += ch.recv(4096)
            for i in range(0, len(data), 32768):
                ch.sendall(data[i : i + 32768])
            ch.shutdown_write()
            end = time.monotonic() + 120
            while not ch.exit_status_ready() and time.monotonic() < end:
                time.sleep(0.05)
        finally:
            ch.close()
        out = self.check(f"/bin/busybox wc -c < {path}.part")
        if str(len(data)) not in out.split():
            raise IOError(f"upload of {path} arrived incomplete")
        self.check(f"chmod {mode} {path}.part && mv {path}.part {path}")

    def close(self):
        self.client.close()


def connect(host, timeout=120, **kwargs):
    """A Shell, retried while the SSH service starts (it takes a few seconds after being enabled)."""
    end = time.monotonic() + timeout
    while True:
        import paramiko
        try:
            return Shell(host, **kwargs)
        except (paramiko.AuthenticationException, paramiko.BadHostKeyException):
            raise
        except Exception as error:  # noqa: BLE001 (refused or reset while the service starts)
            if time.monotonic() > end:
                raise IOError(f"no SSH on {host}: {error}") from None
            time.sleep(5)


def wait_for(what, check, timeout):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        try:
            if check():
                return
        except Exception:  # noqa: BLE001 (SSH and HTTP errors while the panel boots; retried)
            pass
        time.sleep(5)
    raise SystemExit(f"Timed out waiting for {what}; the panel may still be busy, check it before retrying")


def bundle_files(bundle):
    files = {}
    for name in PAYLOAD:
        files[name] = (bundle / "payload" / name).read_bytes()
    for name in THIRD_PARTY:
        files[name] = (bundle / "third-party" / name).read_bytes()
    sums = json.loads((bundle / "third-party" / "SOURCES.json").read_text())
    for entry in sums:
        if hashlib.sha256(files[entry["file"]]).hexdigest() != entry["sha256"]:
            raise SystemExit(f"third-party/{entry['file']} does not match its recorded SHA-256")
    dropbear = bundle / "payload" / "dropbearmulti"
    if not dropbear.exists():
        raise SystemExit("payload/dropbearmulti is missing: without it the panel has no SSH after the switch")
    files["dropbearmulti"] = dropbear.read_bytes()
    return files, (bundle / "payload" / "start.sh").read_bytes()


def main():
    if not __debug__:
        raise SystemExit("Run without -O: validation assertions must remain enabled")
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    for key, question, default, secret in SETTINGS:
        if not secret:
            parser.add_argument(f"--{key.replace('_', '-')}", dest=key, help=question)
    for key in ADVANCED:
        parser.add_argument(f"--{key.replace('_', '-')}", dest=key, help=f"cam.conf {key} (optional)")
    for key in DEPRECATED:
        parser.add_argument(f"--{key.replace('_', '-')}", dest=key, help=f"deprecated: seeds the live {key} once")
    parser.add_argument("--config", help="TOML file with any of the settings above (secrets included)")
    parser.add_argument("--bundle", type=Path, default=HERE, help="release bundle directory (default: this one)")
    parser.add_argument("--work-dir", type=Path, help="where to keep the firmware read and build evidence")
    parser.add_argument("--yes", action="store_true", help="do not ask before flashing and switching")
    args = parser.parse_args()
    v = settings(args)
    files, supervisor = bundle_files(args.bundle)
    conf = cam_conf(v)
    pins, pin_count = pins_conf(v["pins"])
    ssh_key = Path(v["ssh_key"]).expanduser().read_bytes()
    if not ssh_key.startswith((b"ssh-ed25519 ", b"ssh-rsa ", b"ecdsa-sha2-")):
        raise SystemExit(f"{v['ssh_key']} is not an SSH public key")
    private_key = str(Path(v["ssh_key"]).expanduser()).removesuffix(".pub")
    work = args.work_dir or Path(tempfile.mkdtemp(prefix="ds-kd8003-ime1-"))
    work.mkdir(parents=True, exist_ok=True)
    known_hosts = work / "known_hosts"
    host = v["host"]

    say(f"Checking {host} over ISAPI")
    isapi = Isapi(host, v["admin_password"])
    try:
        info = isapi.device()
    except urllib.error.HTTPError as error:
        raise SystemExit(f"ISAPI refused the request ({error.code}): check the address and the admin password")
    if MODEL not in info.get("model", "") or (info.get("firmwareVersion"), info.get("firmwareReleasedDate")) != FIRMWARE:
        raise SystemExit(f"Supported: {MODEL} {' '.join(FIRMWARE)}; this panel reports "
                         f"{info.get('model')} {info.get('firmwareVersion')} {info.get('firmwareReleasedDate')}")
    print(f"    {info.get('model')} {info.get('firmwareVersion')} {info.get('firmwareReleasedDate')}")
    if not isapi.ssh_enabled():
        say("Enabling the stock SSH service")
        isapi.enable_ssh()

    shell = connect(host, password=v["admin_password"], known_hosts=known_hosts)
    marker = shell.run("cat /home/app/custom/boot-marker 2>/dev/null")[1].strip()
    name, size = FIRMWARE_PARTITION
    mtd = shell.check("cat /proc/mtd")
    if not re.search(rf"^{name}: {size:08x} ", mtd, re.M):
        raise SystemExit(f"Unexpected flash layout:\n{mtd}")
    say(f"Reading the firmware partition ({size // 1024} KiB)")
    flash = shell.read(f"/dev/{name}", size)
    (work / "mtd1.bin").write_bytes(flash)
    import build
    import stock
    cramfs = stock.cramfs_from_flash(flash)
    key = stock.keypad_key(cramfs)

    if marker != MARKER:
        say("Rebuilding the stock package from the panel and adding the startup hook")
        dav = stock.stock_dav(cramfs)
        build.build_hook(dav, work / "hook")
        candidate = (work / "hook" / "ds-kd8003-ime1-2.2.62-hook.dav").read_bytes()
        if not args.yes and input("Flash the panel with the startup hook now? [y/N] ").strip().lower() != "y":
            raise SystemExit("Stopped before flashing; nothing on the panel was changed")
        shell.close()
        say("Flashing (the panel reboots; do not cut its power)")
        isapi.flash(candidate)

        def offline():
            try:
                isapi.device()
                return False
            except (OSError, urllib.error.URLError):
                return True

        wait_for("the panel to start its update reboot", offline, 300)
        wait_for("the panel to come back after flashing", lambda: isapi.device(), 600)
        if not isapi.ssh_enabled():
            isapi.enable_ssh()
        shell = connect(host, password=v["admin_password"], known_hosts=known_hosts)
        marker = shell.run("cat /home/app/custom/boot-marker 2>/dev/null")[1].strip()
        if marker != MARKER:
            raise SystemExit("The startup hook did not report after the update; the panel runs stock firmware")
    say("Startup hook installed")

    say(f"Uploading the application ({pin_count} codes)")
    shell.check(f"umask 077; mkdir -p {BASE}/native {BASE}/ssh")
    compressed = {f"native/{n}.lzma": lzma.compress(d, format=lzma.FORMAT_ALONE, preset=9) for n, d in files.items()}
    compressed["native/clips.tar.lzma"] = lzma.compress(clips_tar(v["clips"]), format=lzma.FORMAT_ALONE, preset=9)
    total = sum(len(d) for d in compressed.values()) + len(supervisor)
    # An update replaces files already on the partition: their space comes back, and it must, since the
    # partition cannot hold the old and the new daemon at once.
    replaced = sum(existing_size(shell, f"{BASE}/{p}") for p in [*compressed, "start.sh"])
    free = partition_free(shell)
    if total + CONFIG_PARTITION_FREE_MIN > free + replaced:
        raise SystemExit(f"The config partition has {free} bytes free (+{replaced} replaced); the application needs {total}")
    for path, data in sorted(compressed.items(), key=lambda item: len(item[1])):
        if len(data) + CONFIG_PARTITION_FREE_MIN > partition_free(shell):
            # No room beside the old copy. Interrupted from here on, the supervisor cannot unpack the
            # application and the panel keeps running its stock firmware; installing again completes it.
            shell.check(f"rm -f {BASE}/{path}")
        shell.write(f"{BASE}/{path}", data)
    shell.write(f"{BASE}/start.sh", supervisor, mode="700")
    if shell.run(f"test -e {BASE}/settings.json")[0]:
        # No settings file yet: the live settings still live in the old cam.conf, if any.
        status, old = shell.run(f"cat {BASE}/cam.conf")
        carried = carried_settings(old, v) if status == 0 else {}
        if carried:
            say(f"Keeping {', '.join(sorted(carried))} from the panel's cam.conf (seeded once into settings.json)")
            conf = cam_conf({**v, **carried})
    shell.write(f"{BASE}/cam.conf", conf)
    shell.write(f"{BASE}/pins.conf", pins)
    shell.write(f"{BASE}/keypad.key", key)
    shell.write(f"{BASE}/ssh/authorized_keys", ssh_key)
    del key
    if not args.yes and input("Switch to the native application and reboot now? [y/N] ").strip().lower() != "y":
        shell.close()
        raise SystemExit(f"Uploaded but not switched: touch {BASE}/native.enabled and reboot to switch")
    shell.check(f"cd {BASE} && rm -f native.boots native.ok native.fallback && touch enabled native.enabled && sync")
    try:
        shell.run("reboot", timeout=15)
    except (OSError, TimeoutError):
        pass  # the reboot closes the session before it can answer
    shell.close()

    say("Waiting for the native application (vendor web interface and ISAPI go away)")
    time.sleep(30)

    def healthy():
        # Our Dropbear has its own host key, generated on the first native boot.
        s = Shell(host, key=private_key, known_hosts=work / "known_hosts.native")
        try:
            log = s.run(f"cat {RUNTIME}/cam.log 2>/dev/null")[1]
        finally:
            s.close()
        return all(token in log for token in ("rtsp: serving", "mqtt: connected", "keypad: ready"))

    wait_for("RTSP, MQTT and the keypad to come up", healthy, 600)
    say(f"Done. Streams: rtsp://{host}/main and /sub; Home Assistant finds "
        f"'{v['device_name']}' over MQTT. SSH: ssh root@{host} with your key.")


if __name__ == "__main__":
    try:
        main()
    except (KeyboardInterrupt, EOFError):
        raise SystemExit("\nStopped")
    except Exception as error:  # noqa: BLE001 (one readable line instead of a traceback)
        name = type(error).__name__
        if name == "AuthenticationException":
            raise SystemExit("Install failed: the panel refused the administrator password over SSH") from None
        raise SystemExit(f"Install failed: {error}") from None
