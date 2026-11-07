#!/usr/bin/env python3
"""Build the pinned static ARM926 Dropbear for the DS-KD8003-IME1 locally, without downloads.

Output: <output-dir>/dropbearmulti-arm926 (dropbear + dropbearkey multi-call binary, static musl,
key-only, Ed25519) and manifest.json with sizes and SHA-256 sums.
"""
import argparse
import hashlib
import json
import os
import shutil
import struct
import subprocess
import tarfile
from pathlib import Path

BASE = Path(__file__).resolve().parent
# The firmware workspace's pinned compiler (firmware/scripts/fetch-zig.py), else the one on PATH.
PINNED_ZIG = BASE.parents[1] / 'firmware/.tools/zig/zig'
ZIG = str(PINNED_ZIG) if PINNED_ZIG.is_file() else 'zig'
SOURCE_SHA256 = 'e098034a843699200c8c977a991fff73159735bf795d5f72ef672c41a6b1ae81'
TARGET = 'arm-linux-musleabi'
CPU = 'arm926ej_s'
CFLAGS = '-Oz -fstack-protector-strong -ffunction-sections -fdata-sections -fno-unwind-tables -fno-asynchronous-unwind-tables'
LDFLAGS = '-static -s -Wl,--gc-sections -Wl,-z,relro -Wl,-z,now'


def digest(data):
    return hashlib.sha256(data).hexdigest()


def check_static_arm(data):
    if data[:4] != b'\x7fELF' or data[4] != 1 or data[5] != 1:
        raise ValueError('not an ELF32 little-endian file')
    kind, machine = struct.unpack_from('<HH', data, 16)
    if kind != 2 or machine != 40:
        raise ValueError('not an ARM executable')
    offset = struct.unpack_from('<I', data, 28)[0]
    size, count = struct.unpack_from('<HH', data, 42)
    kinds = [struct.unpack_from('<I', data, offset + i * size)[0] for i in range(count)]
    if 3 in kinds or 2 in kinds:
        raise ValueError('expected a static executable (no PT_INTERP/PT_DYNAMIC)')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source_archive', type=Path)
    parser.add_argument('--output-dir', type=Path, required=True)
    args = parser.parse_args()
    os.umask(0o077)
    if digest(args.source_archive.read_bytes()) != SOURCE_SHA256:
        raise SystemExit('Use the exact Dropbear 2026.94 release tarball')
    if subprocess.check_output([ZIG, 'version'], text=True).strip() != '0.17.0':
        raise SystemExit('Match Zig 0.17.0 for the tested build recipe')
    output = args.output_dir.resolve()
    output.mkdir(mode=0o700)
    with tarfile.open(args.source_archive) as archive:
        archive.extractall(output, filter='data')
    source = output / 'dropbear-2026.94'
    shutil.copyfile(BASE / 'localoptions.h', source / 'localoptions.h')
    cc = f'{ZIG} cc -target {TARGET} -mcpu={CPU}'
    # ARMv5 atomics: the compiler emits __sync_* libcalls that Zig's libc does not provide; see sync_atomics.c.
    atomics = output / 'sync_atomics.o'
    subprocess.run([*cc.split(), *CFLAGS.split(), '-Wall', '-Wextra', '-Werror', '-c', str(BASE / 'sync_atomics.c'), '-o', str(atomics)], check=True)
    configure = ['./configure', f'--host={TARGET}', 'CC=' + cc, f'AR={ZIG} ar', f'RANLIB={ZIG} ranlib',
                 'CFLAGS=' + CFLAGS, 'LDFLAGS=' + LDFLAGS, 'LIBS=' + str(atomics),
                 '--disable-harden', '--disable-zlib', '--disable-syslog', '--disable-utmp',
                 '--disable-utmpx', '--disable-wtmp', '--disable-wtmpx', '--disable-lastlog',
                 '--disable-loginfunc']
    for name, command in [('configure', configure),
                          ('make', ['make', '-j4', 'PROGRAMS=dropbear dropbearkey', 'MULTI=1'])]:
        with (output / (name + '.log')).open('wb') as log:
            subprocess.run(command, cwd=source, stdout=log, stderr=subprocess.STDOUT, check=True)
    worker = output / 'dropbearmulti-arm926'
    shutil.copyfile(source / 'dropbearmulti', worker)
    worker.chmod(0o755)
    check_static_arm(worker.read_bytes())
    report = {'dropbear_version': '2026.94', 'zig_version': '0.17.0', 'source_sha256': SOURCE_SHA256,
              'target': f'{TARGET}/{CPU}', 'cflags': CFLAGS, 'ldflags': LDFLAGS,
              'binaries': {worker.name: {'bytes': worker.stat().st_size, 'sha256': digest(worker.read_bytes())}}}
    (output / 'manifest.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report['binaries']))


if __name__ == '__main__':
    main()
