#!/usr/bin/env python3
"""Unpack the hash-pinned libopus release into this workspace's ignored .tools directory."""
import fcntl
import json
from pathlib import Path
import tarfile
import tempfile
import download

ROOT = Path(__file__).resolve().parents[1]


def main():
    lock = json.loads((ROOT / 'opus.json').read_text())
    output = ROOT / '.tools' / ('opus-' + lock['version'])
    output.parent.mkdir(parents=True, exist_ok=True)
    with (output.parent / '.opus-install.lock').open('a') as guard:
        fcntl.flock(guard, fcntl.LOCK_EX)
        if output.exists():
            receipt = output / '.source-receipt.json'
            if not receipt.is_file() or json.loads(receipt.read_text()).get('archive_sha256') != lock['sha256']:
                raise ValueError('Cached libopus source has no matching receipt; move it aside and run again')
            print(output)
            return
        with tempfile.TemporaryDirectory(prefix='opus-download-', dir=output.parent) as directory:
            temporary = Path(directory)
            archive = temporary / 'opus.tar.gz'
            try:
                archive.write_bytes(download.fetch(lock['url'], lock['sha256']))
            except download.DigestMismatch:
                raise ValueError('libopus archive SHA-256 does not match the recorded release') from None
            with tarfile.open(archive) as bundle:
                bundle.extractall(temporary / 'unpacked', filter='data')
            source = temporary / 'unpacked' / ('opus-' + lock['version'])
            if not (source / 'opus_sources.mk').is_file():
                raise ValueError('Unexpected libopus archive layout')
            (source / '.source-receipt.json').write_text(json.dumps({'archive_sha256': lock['sha256']}, indent=2) + '\n')
            source.rename(output)
        print(output)


if __name__ == '__main__':
    main()
