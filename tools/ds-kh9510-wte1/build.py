#!/usr/bin/env python3
"""Build a local APK using Android API 22 + legacy dx tooling; no downloads."""
import argparse
import base64
from pathlib import Path
import shutil
import struct
import subprocess
import zipfile


def run(*args):
    subprocess.run([str(a) for a in args], check=True)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--android-jar', type=Path, required=True)
    p.add_argument('--dx-jar', type=Path, required=True)
    p.add_argument('--apksigner-jar', type=Path, required=True)
    p.add_argument('--public-key', type=Path, required=True)
    p.add_argument('--aapt-image', default='ds-kh9510-wte1-aapt')
    a = p.parse_args()
    # Accept an ordinary RSA public key, stripping its optional identifying comment.
    fields = a.public_key.read_text().strip().split()
    if len(fields) < 2 or fields[0] != 'ssh-rsa':
        p.error('use an RSA .pub file, without authorized_keys options')
    try:
        raw = base64.b64decode(fields[1], validate=True)
        offset = 0
        parts = []
        for _ in range(3):
            length = struct.unpack_from('>I', raw, offset)[0]
            offset += 4
            parts.append(raw[offset:offset + length])
            if len(parts[-1]) != length:
                raise ValueError('truncated RSA key')
            offset += length
        if offset != len(raw) or parts[0] != b'ssh-rsa' or len(parts[2]) < 256:
            raise ValueError('invalid or undersized RSA key')
    except (ValueError, struct.error) as e:
        p.error(str(e))
    root = Path(__file__).resolve().parents[2]
    source = Path(__file__).resolve().parent
    output = root / 'build/ds-kh9510-wte1'
    if output.exists():
        p.error(f'{output} already exists; move or remove it before rebuilding')
    output.mkdir(parents=True)
    assets = output / 'assets'
    assets.mkdir()
    (assets / 'owner.pub').write_text(' '.join(fields[:2]) + '\n')
    shutil.copyfile(source / 'install-key.sh', assets / 'install-key.sh')
    classes = output / 'classes'
    classes.mkdir()
    run('javac', '-source', '8', '-target', '8', '-bootclasspath', a.android_jar,
        '-d', classes, source / 'src/org/ownyourcameras/ds_kh9510_wte1/MainActivity.java')
    run('java', '-jar', a.dx_jar, '--dex', f'--output={output / "classes.dex"}', classes)
    # Stage android.jar under ignored build/ so Docker needs only the project mount.
    shutil.copyfile(a.android_jar, output / 'android.jar')
    run('docker', 'run', '--rm', '--entrypoint', 'aapt', '-v', f'{root}:/work', '-w', '/work', a.aapt_image,
        'package', '-f', '-M', 'tools/ds-kh9510-wte1/AndroidManifest.xml',
        '-A', 'build/ds-kh9510-wte1/assets', '-I', 'build/ds-kh9510-wte1/android.jar',
        '-F', 'build/ds-kh9510-wte1/unsigned.apk')
    with zipfile.ZipFile(output / 'unsigned.apk', 'a') as apk:
        apk.write(output / 'classes.dex', 'classes.dex')
    # New local development signer; never reuse or distribute a private research key.
    run('keytool', '-genkeypair', '-keystore', output / 'development.jks',
        '-storepass', 'android', '-keypass', 'android', '-alias', 'local',
        '-keyalg', 'RSA', '-keysize', '2048', '-validity', '3650',
        '-dname', 'CN=Local Development', '-storetype', 'JKS')
    run('java', '-jar', a.apksigner_jar, 'sign', '--ks', output / 'development.jks',
        '--ks-pass', 'pass:android', '--key-pass', 'pass:android',
        '--min-sdk-version', '22', '--v1-signing-enabled', 'true',
        '--out', output / 'ds-kh9510-wte1-root-helper.apk', output / 'unsigned.apk')
    run('java', '-jar', a.apksigner_jar, 'verify', output / 'ds-kh9510-wte1-root-helper.apk')
    print(f'Local APK: {output / "ds-kh9510-wte1-root-helper.apk"}')
    print('APK contains your public key. Keep build/ private.')


if __name__ == '__main__':
    main()
