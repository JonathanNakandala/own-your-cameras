#!/usr/bin/env python3
"""Install the hash-pinned Zig compiler into this workspace's ignored .tools directory."""
import fcntl
import hashlib
import json
from pathlib import Path
import platform
import subprocess
import tarfile
import tempfile
import download

ROOT = Path(__file__).resolve().parents[1]


def main():
    lock = json.loads((ROOT / 'toolchain.json').read_text())
    machine = {'arm64': 'aarch64', 'aarch64': 'aarch64', 'AMD64': 'x86_64', 'x86_64': 'x86_64'}[platform.machine()]
    system = {'Darwin': 'macos', 'Linux': 'linux'}[platform.system()]
    source = lock['platforms'][machine + '-' + system]
    output = ROOT / '.tools/zig'
    output.parent.mkdir(parents=True, exist_ok=True)
    with (output.parent / '.zig-install.lock').open('a') as guard:
        fcntl.flock(guard, fcntl.LOCK_EX)
        if output.exists():
            receipt_path = output / '.compiler-receipt.json'
            if not receipt_path.is_file():
                raise ValueError('Cached compiler has no verified receipt; move .tools/zig aside and run again')
            receipt = json.loads(receipt_path.read_text())
            if (receipt.get('archive_sha256') != source['sha256']
                    or receipt.get('binary_sha256') != hashlib.sha256((output / 'zig').read_bytes()).hexdigest()):
                raise ValueError('Cached compiler differs from its verified release receipt')
            if subprocess.check_output([str(output / 'zig'), 'version'], text=True).strip() != lock['version']:
                raise ValueError('Existing compiler differs; choose a clean workspace tools directory')
            print(output)
            return
        with tempfile.TemporaryDirectory(prefix='zig-download-', dir=output.parent) as directory:
            temporary = Path(directory)
            archive = temporary / 'zig.tar.xz'
            try:
                archive.write_bytes(download.fetch(source['url'], source['sha256']))
            except download.DigestMismatch:
                raise ValueError('Compiler archive SHA-256 does not match the recorded official release') from None
            with tarfile.open(archive) as bundle:
                bundle.extractall(temporary / 'unpacked', filter='data')
            candidates = list((temporary / 'unpacked').iterdir())
            if len(candidates) != 1 or not (candidates[0] / 'zig').is_file():
                raise ValueError('Unexpected compiler archive layout')
            if subprocess.check_output([str(candidates[0] / 'zig'), 'version'], text=True).strip() != lock['version']:
                raise ValueError('Unexpected compiler version')
            (candidates[0] / '.compiler-receipt.json').write_text(json.dumps({
                'archive_sha256': source['sha256'],
                'binary_sha256': hashlib.sha256((candidates[0] / 'zig').read_bytes()).hexdigest(),
            }, indent=2) + '\n')
            candidates[0].rename(output)
        print(output)


if __name__ == '__main__':
    main()
