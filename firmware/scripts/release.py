#!/usr/bin/env python3
"""Build model-specific binary/builder releases; private firmware inputs are never accepted."""
import argparse
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess
import tarfile
import tempfile
import urllib.error
import urllib.request

import download

ROOT = Path(__file__).resolve().parents[2]
DROPBEAR_URLS = (
    'https://matt.ucc.asn.au/dropbear/releases/dropbear-2026.94.tar.bz2',
    'https://dropbear.nl/mirror/releases/dropbear-2026.94.tar.bz2',
)
DROPBEAR_SHA256 = 'e098034a843699200c8c977a991fff73159735bf795d5f72ef672c41a6b1ae81'
MODELS = {
    'mi360': ('MJSXJ10CM / T31 / SC2336', '4.3.4_0376'),
    'reolink-e1-zoom': ('IPC_566SD664M5MP / Cortex-A9', 'v3.1.0.4417_2412122178'),
    'ds-kd8003-ime1': ('DS-KD8003-IME1 / Hi3516CV300 / AR0237', 'V2.2.62 build 230204'),
}
# HiSilicon Hi3516CV300 MPP V1 user-space libraries the DS-KD8003-IME1 application loads; the stock
# application links them statically. Fetched from OpenIPC at a pinned commit, shipped as third-party
# files outside the MIT license, never committed here.
OPENIPC_LIBS = 'https://raw.githubusercontent.com/OpenIPC/firmware/d66ec8cadb28c956d8e0af05d1c44db9e14164bc/general/package/hisilicon-osdrv-hi3516cv300/files/lib/'
OPENIPC_SHA256 = {
    'libmpi.so': '79dece1e0f39f0cbccd51ab80eb29c3ee685d769e90bee66c772d75ef1522aef',
    'libVoiceEngine.so': '0096c76fd571541517c3ff03ef2c62ac15919daab4f2f513107f766028764465',
    'libdnvqe.so': 'ebece076b409198e3649f3aa14a5906f0552ae857b551d7fa7c5c219646221e6',
    'libupvqe.so': 'f2010833455ac1f40a03d8ec8a2e3ba7bd45c6c41ee71773f2c6dca328441f95',
}
TARGETS = {'mi360': 'mipsel-unknown-linux-uclibc', 'reolink-e1-zoom': 'armv7-unknown-linux-gnueabihf',
           'ds-kd8003-ime1': 'armv5te-unknown-linux-uclibceabi'}
RUST_PACKAGES = {'mi360': ('mi360-cam',), 'reolink-e1-zoom': ('cam', 'cam-launch'), 'ds-kd8003-ime1': ('ds-kd8003-ime1-cam',)}
BUILD_TARGETS = {'mi360': 'build-mi360', 'reolink-e1-zoom': 'build-reolink', 'ds-kd8003-ime1': 'build-ds-kd8003-ime1'}


def run(*args, **kwargs):
    subprocess.run([str(x) for x in args], check=True, **kwargs)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def download_dropbear(archive):
    failures = []
    for url in DROPBEAR_URLS:
        try:
            data = download.fetch(url, DROPBEAR_SHA256)
        except download.DigestMismatch:
            raise ValueError(f'Dropbear source digest mismatch: {url}') from None
        except (OSError, urllib.error.URLError) as error:
            failures.append(f'{url}: {error}')
            continue
        archive.write_bytes(data)
        return
    raise RuntimeError('Dropbear source download failed: ' + '; '.join(failures))


def download_openipc(directory):
    """Fetch the pinned MPP libraries; returns their SOURCES.json records."""
    records = []
    for name, expected in OPENIPC_SHA256.items():
        try:
            data = download.fetch(OPENIPC_LIBS + name, expected)
        except download.DigestMismatch:
            raise ValueError(f'OpenIPC library digest mismatch: {name}') from None
        (directory / name).write_bytes(data)
        records.append(dict(file=name, sha256=expected, url=OPENIPC_LIBS + name,
                            license='HiSilicon SDK binary redistributed by OpenIPC; not covered by this project\'s MIT license'))
    return records


def check_binary(data, machine):
    if len(data) < 52 or data[:7] != b'\x7fELF\x01\x01\x01' or struct.unpack_from('<H', data, 18)[0] != machine:
        raise ValueError('Wrong binary architecture')
    offset = struct.unpack_from('<I', data, 28)[0]
    width, count = struct.unpack_from('<HH', data, 42)
    if width != 32 or not 1 <= count <= 64 or offset + count * width > len(data):
        raise ValueError('Invalid runtime program headers (link stubs must never be packaged)')
    if not any(struct.unpack_from('<I', data, offset + i * width)[0] == 1 for i in range(count)):
        raise ValueError('Runtime binary has no loadable segments')


def source_paths(model):
    camera = ROOT / 'firmware/cameras' / model
    if model == 'mi360':
        paths = [camera / 'firmware' / n for n in (
            'bootstrap.py', 'build_image.py', 'mtd_writer.py', 'verify_environment.py', 'stock.lock.json',
            'boot-policy.sh', 'launcher.sh', 'network-start.sh', 'S60service', 'S99mi360_network', 'S99zzz_mi360')]
        paths += [camera / 'scripts' / n for n in ('network-ap.sh', 'network-dhcp.sh', 'network-recover.sh', 'network-wpa-event.sh')]
        paths += [camera / 'scripts/recover-network.py']
        paths += [ROOT / 'tools/mi-360' / n for n in ('build.py', 'firmware.py', 'S99zz_rootssh', 'camera-update', 'camera-mtd-write.c')]
    elif model == 'ds-kd8003-ime1':
        paths = []
    else:
        paths = [camera / 'scripts' / n for n in ('cutover_image.py', 'flash_cutover.py', 'own_your_cameras.py', 'spub.py', 'start_app.py', 'udiv-guard')]
        paths += [camera / 'image/start_app']
        paths += [ROOT / 'tools/reolink-e1-zoom' / n for n in ('build.py', 'ubi.py', 'upload.py', 'requirements.txt', 'S99_zzRootSSH')]
    paths += [ROOT / 'firmware/LICENSE', camera / 'README.md']
    paths += sorted((camera / 'docs').rglob('*.md'))
    return paths


def rust_notices(model):
    target = TARGETS[model]
    workspace = ROOT / 'firmware'
    metadata = json.loads(subprocess.check_output(
        ['cargo', 'metadata', '--locked', '--format-version', '1', '--filter-platform', target], cwd=workspace))
    packages = {p['id']: p for p in metadata['packages']}
    nodes = {n['id']: n for n in metadata['resolve']['nodes']}
    names = RUST_PACKAGES[model]
    pending = [p['id'] for p in packages.values() if p['source'] is None and p['name'] in names]
    if not pending: raise ValueError('Missing release packages')
    seen = set()
    while pending:
        package = pending.pop()
        if package in seen: continue
        seen.add(package)
        pending += [d['pkg'] for d in nodes[package]['deps'] if any(k['kind'] != 'dev' for k in d['dep_kinds'])]
    files, records = {}, []
    for package in sorted((packages[i] for i in seen), key=lambda p: (p['name'], p['version'])):
        if package['source'] is None: continue
        root = Path(package['manifest_path']).parent
        notices = sorted(p for p in root.rglob('*') if p.is_file() and
                         p.name.lower().startswith(('license', 'licence', 'copying', 'copyright', 'notice')))
        prefix = 'licenses/rust-crates/' + package['name'] + '-' + package['version'] + '/'
        for path in notices: files[prefix + path.relative_to(root).as_posix()] = path
        if not notices:
            # A published crate without its license file: keep the license from the
            # exact upstream tag in releases/licenses/<name>-<version>-LICENSE.txt.
            fallback = workspace / 'releases/licenses' / (package['name'] + '-' + package['version'] + '-LICENSE.txt')
            if not fallback.is_file(): raise ValueError('Missing dependency license: ' + package['name'])
            files[prefix + 'LICENSE'] = fallback
        records.append({k: package[k] for k in ('name', 'version', 'license', 'repository')})
    sysroot = Path(subprocess.check_output(['rustc', '--print', 'sysroot'], cwd=workspace, text=True).strip())
    rust_docs = sysroot / 'share/doc/rust'
    files['licenses/rust-toolchain/COPYRIGHT-library.html'] = rust_docs / 'COPYRIGHT-library.html'
    for path in sorted((rust_docs / 'licenses').iterdir()):
        if path.is_file(): files['licenses/rust-toolchain/licenses/' + path.name] = path
    return files, records


def package(model, version, workers, writer, output, copyright_file, dropbear_source=None, third_party=None):
    if model not in MODELS or not re.fullmatch(re.escape(model) + r'-v[0-9]+\.[0-9]+\.[0-9]+(?:-[A-Za-z0-9.-]+)?', version):
        raise ValueError('Use a model-specific tag such as mi360-v0.1.0')
    revision = subprocess.check_output(['git', '-C', str(ROOT), 'rev-parse', 'HEAD'], text=True).strip()
    dirty = bool(subprocess.check_output(['git', '-C', str(ROOT), 'status', '--porcelain', '--untracked-files=normal'], text=True))
    epoch = int(subprocess.check_output(['git', '-C', str(ROOT), 'show', '-s', '--format=%ct', 'HEAD'], text=True))
    files = {}
    def add(name, path):
        path = Path(path)
        if path.is_symlink(): raise ValueError('Release inputs must be regular files')
        files[name] = (path.read_bytes(), 0o755 if os.access(path, os.X_OK) else 0o644)
    for path in source_paths(model): add('source/' + path.relative_to(ROOT).as_posix(), path)
    if model == 'ds-kd8003-ime1':
        tools = ROOT / 'tools' / model
        for name in ('install.py', 'build.py', 'stock.py', 'boot-hook.sh', 'requirements.txt'): add(name, tools / name)
    else:
        add('build-image.py', ROOT / 'firmware/scripts/bundle_image.py')
    add('DROPBEAR-LICENSE', copyright_file)
    add('INSTALL.md', ROOT / 'firmware/releases' / (model + '.md.in'))
    add('MUSL-COPYRIGHT', ROOT / 'firmware/.tools/zig/lib/libc/musl/COPYRIGHT')
    opus = 'opus-' + json.loads((ROOT / 'firmware/opus.json').read_text())['version']
    add('licenses/' + opus + '/COPYING', ROOT / 'firmware/.tools' / opus / 'COPYING')
    notices, dependencies = rust_notices(model)
    for name, path in notices.items(): add(name, path)
    files['licenses/rust-dependencies.json'] = ((json.dumps(dependencies, indent=2) + '\n').encode(), 0o644)
    tools = ROOT / 'tools' / ('mi-360' if model == 'mi360' else model)
    for name in ('build_dropbear.py', 'localoptions.h'):
        add('source/' + (tools / name).relative_to(ROOT).as_posix(), tools / name)
    if dropbear_source is not None:
        add('third-party/dropbear-2026.94.tar.bz2', dropbear_source)
    files['source/SOURCE_REVISION'] = ((revision + '\n').encode(), 0o644)
    if model == 'mi360':
        add('bin/mi360-cam', ROOT / 'firmware/cameras/mi360/target/mipsel-unknown-linux-uclibc/release/mi360-cam')
        add('bin/dropbearmulti-mipsel', workers / 'dropbearmulti-mipsel')
        add('bin/bootstrap-camera-mtd-write', workers / 'camera-mtd-write')
        for name in ('camera-mtd-write', 'camera-mtd-write.c', 'camera-update'): add('writer/' + name, writer / name)
        report = json.loads((writer / 'writer-manifest.json').read_text())
        report.pop('command', None)  # Build-machine paths are not release metadata.
        files['writer/writer-manifest.json'] = ((json.dumps(report, indent=2) + '\n').encode(), 0o644)
    elif model == 'ds-kd8003-ime1':
        dist = ROOT / 'firmware/cameras/ds-kd8003-ime1/dist'
        for name in ('ds-kd8003-ime1-cam', 'libloader_compat.so', 'libnative_compat.so', 'libnative_media_group.so'):
            add('payload/' + name, dist / name)
        add('payload/start.sh', ROOT / 'firmware/cameras/ds-kd8003-ime1/firmware/start.sh')
        add('payload/dropbearmulti', workers / 'dropbearmulti-arm926')
        for name in OPENIPC_SHA256:
            add('third-party/' + name, third_party / name)
        records = json.loads((third_party / 'SOURCES.json').read_text())
        files['third-party/SOURCES.json'] = ((json.dumps(records, indent=2) + '\n').encode(), 0o644)
    else:
        for name in ('cam', 'cam-launch'):
            add('bin/' + name, ROOT / 'firmware/cameras/reolink-e1-zoom/target/armv7-unknown-linux-gnueabihf/release' / name)
        add('bin/dropbearmulti-cortex-a9', workers / 'dropbearmulti-cortex-a9')
    for name, (data, _) in files.items():
        elf = name.startswith(('bin/', 'third-party/lib')) or name == 'writer/camera-mtd-write' or (name.startswith('payload/') and name != 'payload/start.sh')
        if elf: check_binary(data, 8 if model == 'mi360' else 40)
    manifest = dict(schema=1, model=model, version=version, source_commit=revision, source_dirty=dirty,
                    hardware=MODELS[model][0], stock_firmware=MODELS[model][1],
                    files={n: digest(data) for n, (data, _) in sorted(files.items())})
    files['manifest.json'] = ((json.dumps(manifest, indent=2) + '\n').encode(), 0o644)
    readme = (f'{version}\n\nRun python3 install.py (see INSTALL.md): it asks for the panel address, the administrator\n'
              'password, MQTT and the PIN table, then installs everything. third-party/ holds HiSilicon libraries\n'
              'redistributed from OpenIPC (see third-party/SOURCES.json); the MIT license does not cover them.\n'
              if model == 'ds-kd8003-ime1' else None)
    files['README.txt'] = (readme.encode(), 0o644) if readme else ((f'{version}\n\nRun python3 build-image.py --help. Supply your own original firmware and SSH public key.\n'
        'Images remain private. This bundle contains no stock firmware, device settings or link-only libraries.\n'
        'Host tools: Python3.12+, SquashFS4.7.5; Reolink also needs ubi-reader and llvm-objdump.\n'
        'Start with INSTALL.md for bundle-specific configuration and installation paths.\n').encode(), 0o644)
    output.mkdir(parents=True, exist_ok=True)
    archive = output / (version + '.tar.gz')
    with archive.open('wb') as raw, gzip.GzipFile(filename='', mode='wb', fileobj=raw, mtime=epoch) as compressed:
        with tarfile.open(fileobj=compressed, mode='w') as tar:
            for name, (data, mode) in sorted(files.items()):
                entry = tarfile.TarInfo(version + '/' + name)
                entry.size, entry.mode, entry.mtime = len(data), mode, epoch
                tar.addfile(entry, io.BytesIO(data))
    (output / (archive.name + '.sha256')).write_text(digest(archive.read_bytes()) + '  ' + archive.name + '\n')
    print(archive)
    return archive


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('model', choices=MODELS)
    p.add_argument('--version', required=True)
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--dropbear-source', type=Path, help='cached pinned source archive; otherwise downloaded with hash verification')
    args = p.parse_args()
    env = os.environ.copy()
    env['PATH'] = str(ROOT / 'firmware/.tools/zig') + ':' + env['PATH']
    run('make', '-C', ROOT / 'firmware', BUILD_TARGETS[args.model], env=env)
    with tempfile.TemporaryDirectory(prefix='oyc-release-') as directory:
        temporary = Path(directory)
        archive = args.dropbear_source
        if archive is None:
            archive = temporary / 'dropbear.tar.bz2'
            download_dropbear(archive)
        if digest(archive.read_bytes()) != DROPBEAR_SHA256: raise ValueError('Dropbear source digest mismatch')
        tools = ROOT / 'tools' / ('mi-360' if args.model == 'mi360' else args.model)
        run('python3', tools / 'build_dropbear.py', archive, '--output-dir', temporary / 'workers', env=env)
        writer = temporary / 'qualified-writer'
        if args.model == 'mi360':
            run('python3', ROOT / 'firmware/cameras/mi360/firmware/mtd_writer.py', '--output', writer, env=env)
        third_party = None
        if args.model == 'ds-kd8003-ime1':
            third_party = temporary / 'third-party'
            third_party.mkdir()
            (third_party / 'SOURCES.json').write_text(json.dumps(download_openipc(third_party), indent=2) + '\n')
        license_path = temporary / 'workers/dropbear-2026.94/LICENSE'
        package(args.model, args.version, temporary / 'workers', writer, args.output.resolve(), license_path, archive, third_party)


if __name__ == '__main__': main()
