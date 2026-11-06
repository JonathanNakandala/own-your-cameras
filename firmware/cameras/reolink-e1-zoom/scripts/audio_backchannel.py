#!/usr/bin/env python3 -I
"""Send an existing real recording to cam's Opus backchannel; never open a microphone.

Convert a real recording first: ffmpeg -i speech.wav -ac 1 -ar 48000 -c:a libopus -b:a 64k speech.opus
The camera must already be running cam serve. This tool changes nothing else.
"""
import argparse
import hashlib
from pathlib import Path
import re
import socket
import struct
import time


def ogg_opus_packets(data):
    """The audio packets of an Ogg Opus file (RFC 7845), without the OpusHead and OpusTags headers."""
    packets, partial, offset = [], b'', 0
    while offset < len(data):
        if data[offset:offset + 4] != b'OggS' or len(data) < offset + 27:
            raise ValueError('not an Ogg stream')
        count = data[offset + 26]
        lacing = data[offset + 27:offset + 27 + count]
        body = offset + 27 + count
        for size in lacing:
            partial += data[body:body + size]
            body += size
            if size < 255:
                packets.append(partial)
                partial = b''
        offset = body
    if len(packets) < 2 or not packets[0].startswith(b'OpusHead') or not packets[1].startswith(b'OpusTags'):
        raise ValueError('not an Ogg Opus stream')
    return packets[2:]


def duration(packet):
    """Samples at 48 kHz in one Opus packet, from its TOC byte (RFC 6716 section 3.1)."""
    config, code = packet[0] >> 3, packet[0] & 3
    if config < 12:
        frame = (480, 960, 1920, 2880)[config % 4]
    elif config < 16:
        frame = (480, 960)[config % 2]
    else:
        frame = (120, 240, 480, 960)[config % 4]
    frames = 1 if code == 0 else 2 if code < 3 else packet[1] & 0x3f
    return frame * frames


def send_opus(host, port, path, packets, credentials=None):
    """Pace existing Opus packets in real time on the backchannel (PT 111, 48 kHz clock). Return packet count."""
    if not packets:
        raise ValueError('recording is empty')
    url = f'rtsp://{host}:{port}/{path}'
    with socket.create_connection((host, port), timeout=5) as sock, sock.makefile('rb') as stream:
        sock.settimeout(10)  # Camera DESCRIBE may wait five seconds for its first video keyframe.
        sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        cseq, digest = 0, None

        def call(method, target, extra=''):
            nonlocal cseq, digest
            for attempt in range(2):
                cseq += 1
                auth = ''
                if digest and credentials:
                    realm, nonce = digest
                    user, password = credentials
                    md5 = lambda value: hashlib.md5(value.encode()).hexdigest()
                    response = md5(f'{md5(f"{user}:{realm}:{password}")}:{nonce}:{md5(f"{method}:{target}")}')
                    auth = f'Authorization: Digest username="{user}", realm="{realm}", nonce="{nonce}", uri="{target}", response="{response}"\r\n'
                sock.sendall(f'{method} {target} RTSP/1.0\r\nCSeq: {cseq}\r\n{auth}{extra}\r\n'.encode())
                # Bound the whole response before allocating a body or buffering headers.
                limit = 16 * 1024
                used = 0

                def line():
                    nonlocal used
                    value = stream.readline(limit - used + 1)
                    used += len(value)
                    if used > limit or not value.endswith(b'\r\n'):
                        raise ValueError('invalid or oversized RTSP response headers')
                    return value

                status = line().decode().strip()
                headers = {}
                while (value := line()) != b'\r\n':
                    key, value = value.decode().split(':', 1)
                    key = key.lower()
                    if key == 'content-length' and key in headers:
                        raise ValueError('duplicate RTSP response content length')
                    headers[key] = value.strip()
                length = headers.get('content-length', '0')
                if not length.isascii() or not length.isdecimal() or len(length) > 5:
                    raise ValueError('invalid RTSP response content length')
                length = int(length)
                if used + length > limit:
                    raise ValueError('oversized RTSP response body')
                if len(stream.read(length)) != length:
                    raise EOFError('camera closed during RTSP response body')
                if ' 200 ' in status:
                    return headers
                if ' 401 ' in status and credentials and attempt == 0:
                    fields = dict(re.findall(r'(\w+)="([^"]*)"', headers.get('www-authenticate', '')))
                    digest = fields['realm'], fields['nonce']
                    continue
                raise RuntimeError(f'{method}: {status}')
            raise RuntimeError('authentication failed')

        call('DESCRIBE', url, 'Require: www.onvif.org/ver20/backchannel\r\nAccept: application/sdp\r\n')
        headers = call('SETUP', url + '/trackID=2', 'Transport: RTP/AVP/TCP;unicast;interleaved=4-5\r\n')
        session = headers['session'].split(';')[0]
        call('PLAY', url, f'Session: {session}\r\n')
        start = time.monotonic()
        clock = 0
        for sequence, payload in enumerate(packets):
            time.sleep(max(0, start + clock / 48000 - time.monotonic()))
            rtp = struct.pack('!BBHII', 0x80, 111, sequence & 65535, clock & 0xffffffff, 31) + payload
            sock.sendall(b'$\x04' + struct.pack('!H', len(rtp)) + rtp)
            clock += duration(payload)
        cseq += 1
        sock.sendall(f'TEARDOWN {url} RTSP/1.0\r\nCSeq: {cseq}\r\nSession: {session}\r\n\r\n'.encode())
        return len(packets)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--host', required=True, help="the camera's address")
    parser.add_argument('--port', type=int, default=554)
    parser.add_argument('--path', choices=['Preview_01_main', 'Preview_01_sub'], default='Preview_01_sub')
    parser.add_argument('--auth', type=Path, help='file containing existing user:password')
    parser.add_argument('opus', type=Path, help='Ogg Opus file made from an existing real recording')
    args = parser.parse_args()
    credentials = tuple(args.auth.read_text().strip().split(':', 1)) if args.auth else None
    packets = ogg_opus_packets(args.opus.read_bytes())
    sent = send_opus(args.host, args.port, args.path, packets, credentials)
    print(f'opus_packets={sent} seconds={sum(map(duration, packets)) / 48000:.3f}')


if __name__ == '__main__':
    main()
