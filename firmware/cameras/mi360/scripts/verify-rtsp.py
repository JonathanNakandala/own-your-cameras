#!/usr/bin/env python3
"""Verify actual Mi360 RTSP media without exposing credentials.

Default operation only listens. --replay-microphone sends at most one second of
the real PCMU captured during this invocation to the negotiated backchannel.
It never generates microphone samples, tones or replacement video.
"""
from __future__ import annotations

import argparse
import base64
import binascii
import hashlib
import ipaddress
import json
import math
import os
from pathlib import Path
import re
import secrets
import select
import socket
import struct
import subprocess
import sys
import time
import tomllib

MIN_CAPTURE_SECONDS = 1.5
# Both real SDK probe recordings put IDRs at frames0 and60 (~3s at20fps).
# A new canonical hub subscriber correctly waits for the next real IDR.
MAX_CAPTURE_SECONDS = 3.5


class VerificationError(Exception):
    """Only constant, credential-free diagnostics may be passed to this class."""


class Limits:
    def __init__(self) -> None:
        self.deadline = time.monotonic() + 15
        self.wire_bytes = 0

    def remaining(self) -> float:
        remaining = self.deadline - time.monotonic()
        if remaining <= 0:
            raise VerificationError("overall verification deadline exceeded")
        return remaining

    def incoming(self, count: int) -> None:
        self.wire_bytes += count
        if self.wire_bytes > 2 * 1024 * 1024:
            raise VerificationError("incoming media exceeded the2MiB budget")


class Credentials:
    __slots__ = ("username", "password")

    def __init__(self, username: str, password: str) -> None:
        self.username, self.password = username, password


def credentials(args: argparse.Namespace, limits: Limits) -> Credentials:
    if args.stdin_config:
        chunks = bytearray()
        while True:
            ready, _, _ = select.select([sys.stdin.buffer], [], [], limits.remaining())
            if not ready:
                raise VerificationError("configuration input deadline exceeded")
            data = os.read(sys.stdin.fileno(), 4096)
            if not data:
                break
            chunks.extend(data)
            if len(chunks) > 16384:
                raise VerificationError("private configuration exceeds16KiB")
        content = bytes(chunks)
    else:
        try:
            result = subprocess.run(
                ["sops", "--decrypt", "--input-type", "json", "--output-type", "binary", str(args.secret)],
                stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                timeout=min(5, limits.remaining()), check=False,
            )
        except (OSError, subprocess.TimeoutExpired):
            raise VerificationError("private configuration decryption failed") from None
        if result.returncode != 0 or len(result.stdout) > 16384:
            raise VerificationError("private configuration decryption failed")
        content = result.stdout
    try:
        value = tomllib.loads(content.decode("utf-8"))["rtsp"]
        username, password = value["username"], value["password"]
        if not isinstance(username, str) or not isinstance(password, str):
            raise ValueError
        if not username or not password or max(len(username), len(password)) > 128:
            raise ValueError
        if any(ord(c) < 32 or ord(c) == 127 for c in username + password):
            raise ValueError
    except (ValueError, KeyError, TypeError):
        raise VerificationError("private RTSP configuration is invalid") from None
    return Credentials(username, password)


def md5(value: str) -> str:
    return hashlib.md5(value.encode("utf-8"), usedforsecurity=False).hexdigest()


def quote(value: str) -> str:
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'


class Client:
    def __init__(self, host: str, port: int, credential: Credentials, limits: Limits) -> None:
        self.limits = limits
        self.credential = credential
        self.socket = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.socket.settimeout(min(3, limits.remaining()))
        self.socket.connect((host, port))
        self.buffer = bytearray()
        self.cseq = 0
        self.challenge: dict[str, str] | None = None
        self.session: str | None = None
        self.record = None

    def close(self) -> None:
        self.socket.close()

    def event(self, wait: float) -> tuple | None:
        until = time.monotonic() + min(wait, self.limits.remaining())
        while True:
            if self.buffer.startswith(b"$") and len(self.buffer) >= 4:
                count = int.from_bytes(self.buffer[2:4], "big")
                if len(self.buffer) >= count + 4:
                    channel, data = self.buffer[1], bytes(self.buffer[4:count + 4])
                    del self.buffer[:count + 4]
                    return ("rtp", channel, data)
            elif self.buffer and not self.buffer.startswith(b"$"):
                if len(self.buffer) >= 5 and not self.buffer.startswith(b"RTSP/"):
                    raise VerificationError("unexpected RTSP framing")
                end = self.buffer.find(b"\r\n\r\n")
                if end > 16384 or (end < 0 and len(self.buffer) > 16384):
                    raise VerificationError("RTSP response header exceeds bound")
                if end >= 0:
                    try:
                        lines = bytes(self.buffer[:end]).decode("utf-8").split("\r\n")
                        status = int(lines[0].split()[1])
                        headers = {}
                        for line in lines[1:]:
                            key, value = line.split(":", 1)
                            headers[key.lower()] = value.strip()
                        count = int(headers.get("content-length", "0"))
                    except (ValueError, IndexError, UnicodeError):
                        raise VerificationError("malformed RTSP response") from None
                    if not 0 <= count <= 16384:
                        raise VerificationError("RTSP response body exceeds bound")
                    if len(self.buffer) >= end + 4 + count:
                        body = bytes(self.buffer[end + 4:end + 4 + count])
                        del self.buffer[:end + 4 + count]
                        return ("response", status, headers, body)
            remaining = min(until - time.monotonic(), self.limits.remaining())
            if remaining <= 0:
                return None
            ready, _, _ = select.select([self.socket], [], [], remaining)
            if not ready:
                return None
            data = self.socket.recv(65536)
            if not data:
                raise VerificationError("RTSP connection closed before verification finished")
            self.limits.incoming(len(data))
            self.buffer.extend(data)
            if len(self.buffer) > 128 * 1024:
                raise VerificationError("RTSP buffered data exceeded bound")

    def authentication(self, method: str, uri: str) -> str | None:
        if self.challenge is None:
            return None
        challenge = self.challenge
        realm, nonce = challenge["realm"], challenge["nonce"]
        a1 = md5(f"{self.credential.username}:{realm}:{self.credential.password}")
        a2 = md5(f"{method}:{uri}")
        response = md5(f"{a1}:{nonce}:{a2}")
        fields = {"username": self.credential.username, "realm": realm, "nonce": nonce,
                  "uri": uri, "response": response}
        return "Digest " + ", ".join(key + "=" + quote(value) for key, value in fields.items())

    def request(self, method: str, uri: str, extra: dict[str, str] | None = None) -> tuple[dict[str, str], bytes]:
        for attempt in range(2):
            self.cseq += 1
            headers = {"CSeq": str(self.cseq)}
            if self.session:
                headers["Session"] = self.session
            if extra:
                headers.update(extra)
            authentication = self.authentication(method, uri)
            if authentication:
                headers["Authorization"] = authentication
            request = f"{method} {uri} RTSP/1.0\r\n" + "".join(f"{key}: {value}\r\n" for key, value in headers.items()) + "\r\n"
            self.socket.settimeout(min(2, self.limits.remaining()))
            self.socket.sendall(request.encode("utf-8"))
            while True:
                event = self.event(min(3, self.limits.remaining()))
                if event is None:
                    raise VerificationError("RTSP response deadline exceeded")
                if event[0] == "rtp":
                    if self.record is not None:
                        self.record.packet(event[1], event[2])
                    continue
                _, status, response_headers, body = event
                if response_headers.get("cseq") != str(self.cseq):
                    raise VerificationError("RTSP response sequence mismatch")
                break
            if status == 401 and attempt == 0:
                raw = response_headers.get("www-authenticate", "")
                if not raw.startswith("Digest ") or len(raw) > 1024:
                    raise VerificationError("RTSP digest challenge is invalid")
                values = {}
                for match in re.finditer(r'(\w+)=(?:"((?:\\.|[^"\\])*)"|([^,\s]+))', raw[7:]):
                    values[match[1].lower()] = re.sub(r"\\(.)", r"\1", match[2]) if match[2] is not None else match[3]
                if not values.get("realm") or not values.get("nonce") or "qop" in values or values.get("algorithm", "MD5").upper() != "MD5":
                    raise VerificationError("unsupported RTSP digest challenge")
                self.challenge = values
                continue
            if status != 200:
                raise VerificationError("RTSP server rejected verification request")
            if "session" in response_headers:
                self.session = response_headers["session"].split(";", 1)[0]
            return response_headers, body
        raise VerificationError("RTSP digest authentication failed")

    def send_audio(self, channel: int, sequence: int, timestamp: int, ssrc: int, payload: bytes) -> None:
        packet = struct.pack("!BBHII", 0x80, 0x80 if sequence == 0 else 0, sequence, timestamp, ssrc) + payload
        self.socket.settimeout(min(1, self.limits.remaining()))
        self.socket.sendall(b"$" + bytes([channel]) + len(packet).to_bytes(2, "big") + packet)


def sdp_media(body: bytes) -> list[dict]:
    try:
        lines = body.decode("utf-8").splitlines()
    except UnicodeError:
        raise VerificationError("SDP is not UTF-8") from None
    media = []
    for line in lines:
        if line.startswith("m="):
            fields = line[2:].split()
            if len(fields) < 4:
                raise VerificationError("malformed SDP media")
            media.append({"kind": fields[0], "payloads": fields[3:], "maps": {}, "fmtp": {}, "direction": "sendrecv"})
        elif media and line.startswith("a=rtpmap:"):
            key, value = line[9:].split(" ", 1)
            media[-1]["maps"][key] = value.upper()
        elif media and line.startswith("a=fmtp:"):
            key, value = line[7:].split(" ", 1)
            media[-1]["fmtp"][key] = value
        elif media and line.startswith("a=control:"):
            media[-1]["control"] = line[10:]
        elif media and line[2:] in ("recvonly", "sendonly", "sendrecv", "inactive"):
            media[-1]["direction"] = line[2:]
    return media


def payload(packet: bytes) -> tuple[int, int, int, bool, bytes]:
    if len(packet) < 12 or packet[0] >> 6 != 2:
        raise VerificationError("malformed real RTP packet")
    start = 12 + (packet[0] & 15) * 4
    if packet[0] & 16:
        if len(packet) < start + 4:
            raise VerificationError("short RTP extension")
        start += 4 + int.from_bytes(packet[start + 2:start + 4], "big") * 4
    padding = packet[-1] if packet[0] & 32 else 0
    if packet[0] & 32 and padding == 0:
        raise VerificationError("invalid RTP padding")
    end = len(packet) - padding
    if not start <= end:
        raise VerificationError("RTP payload bounds are invalid")
    return packet[1] & 127, int.from_bytes(packet[2:4], "big"), int.from_bytes(packet[4:8], "big"), bool(packet[1] & 128), packet[start:end]


class Clock:
    def __init__(self) -> None:
        self.packets = 0
        self.last_sequence = None
        self.last_timestamp = None
        self.advances = 0
        self.span = 0
        self.monotonic = True

    def observe(self, sequence: int, timestamp: int) -> None:
        if self.last_sequence is not None and sequence != (self.last_sequence + 1) & 65535:
            self.monotonic = False
            raise VerificationError("real RTP sequence is discontinuous")
        if self.last_timestamp is not None:
            delta = (timestamp - self.last_timestamp) & 0xffffffff
            if delta >= 0x80000000:
                self.monotonic = False
                raise VerificationError("real RTP timestamp regressed")
            if delta:
                self.advances += 1
                self.span += delta
        self.last_sequence, self.last_timestamp = sequence, timestamp
        self.packets += 1


class Recording:
    def __init__(self, sps: bytes, pps: bytes) -> None:
        self.sps, self.pps = sps, pps
        self.video = bytearray()
        self.audio = bytearray()
        self.video_clock, self.audio_clock = Clock(), Clock()
        self.nals: list[bytes] = []
        self.fragment = None
        self.started = False
        self.frames = 0
        self.saved_frames = 0
        self.key_frames = 0

    def nal(self, data: bytes) -> None:
        if not data or len(data) > 1024 * 1024 or len(self.nals) >= 256:
            raise VerificationError("H264 NAL bounds are invalid")
        self.nals.append(data)

    def packet(self, channel: int, packet: bytes) -> None:
        if channel not in (0, 2):
            return
        pt, sequence, timestamp, marker, data = payload(packet)
        if channel == 2:
            if pt != 0 or not data:
                raise VerificationError("microphone RTP is not PCMU")
            self.audio_clock.observe(sequence, timestamp)
            self.audio.extend(data)
            return
        if pt != 96 or not data:
            raise VerificationError("video RTP is not negotiated H264")
        self.video_clock.observe(sequence, timestamp)
        kind = data[0] & 31
        if 1 <= kind <= 23:
            if self.fragment is not None:
                raise VerificationError("incomplete H264 fragment")
            self.nal(data)
        elif kind == 28 and len(data) >= 3:
            start, end = bool(data[1] & 128), bool(data[1] & 64)
            if start:
                if self.fragment is not None:
                    raise VerificationError("overlapping H264 fragments")
                self.fragment = bytearray([(data[0] & 224) | (data[1] & 31)])
            if self.fragment is None:
                raise VerificationError("H264 fragment arrived without start")
            self.fragment.extend(data[2:])
            if len(self.fragment) > 1024 * 1024:
                raise VerificationError("H264 fragment exceeds frame budget")
            if end:
                self.nal(bytes(self.fragment))
                self.fragment = None
        else:
            raise VerificationError("unsupported H264 RTP packetization")
        if marker:
            if self.fragment is not None:
                raise VerificationError("H264 marker precedes complete frame")
            self.frames += 1
            key = any(nal[0] & 31 == 5 for nal in self.nals)
            if key:
                self.key_frames += 1
            if key and not self.started:
                types = {nal[0] & 31 for nal in self.nals}
                for number, parameter in [(7, self.sps), (8, self.pps)]:
                    if number not in types:
                        self.video.extend(b"\x00\x00\x00\x01" + parameter)
                self.started = True
            if self.started:
                for nal in self.nals:
                    self.video.extend(b"\x00\x00\x00\x01" + nal)
                self.saved_frames += 1
            self.nals.clear()
        if len(self.audio) + len(self.video) > 2 * 1024 * 1024:
            raise VerificationError("recorded media exceeds2MiB")

    def report(self) -> dict:
        samples = []
        for byte in self.audio:
            value = (~byte) & 255
            magnitude = (((value & 15) * 8 + 132) << ((value >> 4) & 7)) - 132
            samples.append(-magnitude if value & 128 else magnitude)
        return {"video_codec": "H264", "audio_codec": "PCMU", "audio_sample_rate": 8000,
                "video_packets": self.video_clock.packets, "video_frames": self.frames,
                "saved_video_frames": self.saved_frames, "key_frames": self.key_frames,
                "video_timestamp_advances": self.video_clock.advances,
                "audio_packets": self.audio_clock.packets,
                "audio_timestamp_advances": self.audio_clock.advances,
                "annexb_bytes": len(self.video), "pcmu_bytes": len(self.audio),
                "audio_peak": max((abs(sample) for sample in samples), default=0),
                "audio_rms": math.sqrt(sum(sample * sample for sample in samples) / len(samples)) if samples else None,
                "audio_nonzero_samples": sum(sample != 0 for sample in samples),
                "timestamp_and_sequence_monotonic": self.video_clock.monotonic and self.audio_clock.monotonic and self.video_clock.packets > 1 and self.audio_clock.packets > 1}

    def ready(self) -> bool:
        return (self.saved_frames >= 2 and self.key_frames >= 1
                and self.video_clock.advances >= 1 and self.audio_clock.packets >= 2
                and self.audio_clock.advances >= 1)


def capture_finished(recording: Recording, elapsed: float) -> bool:
    return elapsed >= MIN_CAPTURE_SECONDS and recording.ready()


def stream(host: str, port: int, path: str, credential: Credentials, limits: Limits, replay: bool) -> tuple[Recording, dict]:
    client = Client(host, port, credential, limits)
    uri = f"rtsp://{host}:{port}/{path}"
    played = False
    replayed = 0
    replay_capture_start = None
    replay_reference_sha256 = None
    recording = None
    try:
        _, body = client.request("DESCRIBE", uri, {"Accept": "application/sdp", **({"Require": "www.onvif.org/ver20/backchannel"} if replay else {})})
        media = sdp_media(body)
        video = next((item for item in media if item["kind"] == "video" and item["maps"].get("96") == "H264/90000"), None)
        audio = next((item for item in media if item["kind"] == "audio" and item["maps"].get("0") == "PCMU/8000/1" and item["direction"] == "recvonly"), None)
        talk = next((item for item in media if item["kind"] == "audio" and item["maps"].get("0") == "PCMU/8000/1" and item["direction"] == "sendonly"), None)
        if video is None or audio is None or (replay and talk is None):
            raise VerificationError("real SDP is missing required H264/PCMU tracks")
        try:
            parameters = re.search(r"(?:^|;)\s*sprop-parameter-sets=([^;\s]+)", video["fmtp"]["96"])[1].split(",")
            sps, pps = (base64.b64decode(value, validate=True) for value in parameters)
            if not sps or not pps or sps[0] & 31 != 7 or pps[0] & 31 != 8 or max(len(sps), len(pps)) > 4096:
                raise ValueError
        except (KeyError, TypeError, ValueError, binascii.Error):
            raise VerificationError("real SDP parameter sets are invalid") from None
        recording = Recording(sps, pps)
        for item, channel in [(video, 0), (audio, 2)] + ([(talk, 4)] if replay else []):
            if item.get("control") not in ("trackID=0", "trackID=1", "trackID=2"):
                raise VerificationError("unsupported real SDP track control")
            client.request("SETUP", uri + "/" + item["control"], {"Transport": f"RTP/AVP/TCP;unicast;interleaved={channel}-{channel + 1}"})
        client.request("PLAY", uri)
        played = True
        client.record = recording
        capture_started = time.monotonic()
        capture_until = capture_started + MAX_CAPTURE_SECONDS
        while time.monotonic() < capture_until:
            event = client.event(min(0.1, capture_until - time.monotonic()))
            if event is not None and event[0] == "rtp":
                recording.packet(event[1], event[2])
            if capture_finished(recording, time.monotonic() - capture_started):
                break
        if recording.saved_frames < 2 or not recording.key_frames or recording.video_clock.advances < 1:
            raise VerificationError("real video producer did not deliver an advancing decodable keyframe")
        if recording.audio_clock.packets < 2 or recording.audio_clock.advances < 1:
            raise VerificationError("real microphone producer did not deliver advancing PCMU")
        if replay:
            actual = bytes(recording.audio[:8000])
            actual = actual[:len(actual) // 160 * 160]
            if not actual:
                raise VerificationError("no real captured microphone data is available for replay")
            replay_capture_start = len(recording.audio)
            replay_reference_sha256 = hashlib.sha256(actual).hexdigest()
            ssrc = secrets.randbits(32)
            next_packet = time.monotonic()
            for sequence, offset in enumerate(range(0, len(actual), 160)):
                while time.monotonic() < next_packet:
                    event = client.event(min(0.02, next_packet - time.monotonic()))
                    if event is not None and event[0] == "rtp":
                        recording.packet(event[1], event[2])
                client.send_audio(4, sequence, offset, ssrc, actual[offset:offset + 160])
                replayed += 160
                next_packet += 0.020
            # Capture the real microphone tail for optional acoustic analysis.
            tail_until = time.monotonic() + 0.25
            while time.monotonic() < tail_until:
                event = client.event(max(0.0, min(0.02, tail_until - time.monotonic())))
                if event is not None and event[0] == "rtp":
                    recording.packet(event[1], event[2])
        client.request("TEARDOWN", uri)
        played = False
        return recording, {**recording.report(), "verified": True, "replayed_real_microphone_bytes": replayed,
                           "replayed_seconds": replayed / 8000, "replay_capture_start_sample": replay_capture_start,
                           "replay_reference_sha256": replay_reference_sha256, "teardown_confirmed": True}
    except VerificationError as error:
        error.recording = recording
        raise
    finally:
        if played:
            try:
                client.request("TEARDOWN", uri)
            except (VerificationError, OSError):
                pass
        client.close()


def private_file(directory: Path, name: str, content: bytes) -> None:
    descriptor = os.open(directory / name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, "wb") as output:
        output.write(content)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", required=True)
    parser.add_argument("--port", type=int, default=554)
    parser.add_argument("--secret", type=Path, help="private SOPS configuration, or use --stdin-config")
    parser.add_argument("--stdin-config", action="store_true", help="read private TOML bytes from stdin instead of SOPS")
    parser.add_argument("--output-dir", required=True, type=Path, help="new/private research directory for actual media and JSON")
    parser.add_argument("--replay-microphone", action="store_true", help="send only this run's real captured PCMU back, at most one second")
    args = parser.parse_args()
    if not args.stdin_config and args.secret is None:
        parser.error("provide --secret or --stdin-config")
    try:
        args.host = str(ipaddress.IPv4Address(args.host))
    except ipaddress.AddressValueError:
        parser.error("host must be an IPv4 address so verification never waits for DNS")
    if not 1 <= args.port <= 65535:
        parser.error("invalid TCP port")
    os.umask(0o077)
    limits = Limits()
    result = {"success": False, "microphone_replay_requested": args.replay_microphone,
              "recordings_are_real_camera_media": True, "streams": {}}
    output_ready = False
    saved_bytes = 0
    try:
        args.output_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
        mode = args.output_dir.lstat()
        if args.output_dir.is_symlink() or not args.output_dir.is_dir() or mode.st_uid != os.getuid() or mode.st_mode & 0o077:
            raise VerificationError("research output directory must be private and owned by this user")
        output_ready = True
        credential = credentials(args, limits)
        for name, path in [("main", "main"), ("sub", "sub")]:
            recording, report = stream(args.host, args.port, path, credential, limits, args.replay_microphone and name == "main")
            saved_bytes += len(recording.video) + len(recording.audio)
            if saved_bytes > 2 * 1024 * 1024:
                raise VerificationError("actual recording artifacts exceed2MiB")
            private_file(args.output_dir, name + ".h264", bytes(recording.video))
            private_file(args.output_dir, name + ".pcmu", bytes(recording.audio))
            result["streams"][name] = report
        result["success"] = True
        result["wire_bytes"] = limits.wire_bytes
    except VerificationError as error:
        result["error"] = str(error)
        recording = getattr(error, "recording", None)
        if recording is not None:
            result["streams"][name] = {**recording.report(), "verified": False}
            try:
                if saved_bytes + len(recording.video) + len(recording.audio) > 2 * 1024 * 1024:
                    raise OSError("recording budget")
                if recording.video:
                    private_file(args.output_dir, name + ".h264", bytes(recording.video))
                if recording.audio:
                    private_file(args.output_dir, name + ".pcmu", bytes(recording.audio))
            except OSError:
                result["partial_capture_write_failed"] = True
    except (OSError, ValueError, TypeError):
        result["error"] = "verification I/O or protocol parsing failed"
    encoded = (json.dumps(result, indent=2) + "\n").encode()
    if not output_ready:
        print(encoded.decode(), end="")
        return 1
    try:
        private_file(args.output_dir, "verification.json", encoded)
    except OSError:
        print("Verification stopped: private report could not be written.", file=sys.stderr)
        return 1
    print(encoded.decode(), end="")
    return 0 if result["success"] else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        print("Verification interrupted; no successful result recorded.", file=sys.stderr)
        sys.exit(130)
