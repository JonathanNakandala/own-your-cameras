# Binary releases

Download the bundle for your exact model from [Releases](https://github.com/jaxer/own-your-cameras/releases),
verify its checksum, then unpack it. Bundles contain prebuilt daemons, SSH/update helpers and the local
image builder, with project, Rust dependency/toolchain, Dropbear and musl license notices.
They contain no stock firmware, settings, owner key or generated link-only library.

```sh
sha256sum -c MODEL-vVERSION.tar.gz.sha256
tar -xzf MODEL-vVERSION.tar.gz
cd MODEL-vVERSION
cat INSTALL.md
```

On macOS, use `shasum -a 256 -c` instead of `sha256sum -c`.
Install Python3.12+ and SquashFS tools4.7.5. Reolink also needs `ubi-reader` from its bundled
`source/tools/reolink-e1-zoom/requirements.txt` and `llvm-objdump` on PATH. Rust, Zig and camera access
are not needed to assemble an image from a release.

## Mi360

Only MJSXJ10CM / T31 / SC2336 with the qualified4.3.4_0376 base is supported.

```sh
python3 build-image.py --original-dump /path/to/original-dump.bin \
  --expected-original-sha256 YOUR_RECORDED_ORIGINAL_SHA256 \
  --authorized-key /path/to/key.pub --output /path/to/private-image
```

Use the [inactive-slot installation procedure](cameras/mi360/docs/install.md) with the generated kernel/
USRFS pair and bundled `writer/camera-mtd-write` and `writer/camera-update`. Preserve the active recovery
slot and live DATA. The full-chip programmer output is private and is not an ordinary update image.

## Reolink E1 Zoom

Only IPC_566SD664M5MP / stock4417 is supported.

```sh
python3 build-image.py --stock-pak /path/to/stock4417.pak \
  --authorized-key /path/to/key.pub --output /path/to/private-image
python3 source/firmware/cameras/reolink-e1-zoom/scripts/flash_cutover.py \
  --own-your-cameras "$PWD/source" --image-dir /path/to/private-image
```

The last command validates and prints the plan without contacting a camera. Follow the
[installation guide](cameras/reolink-e1-zoom/docs/install.md) before executing it: the vendor updater
must be running, and configuration retention/recovery requirements still apply. Never run a second
media daemon alongside the existing owner.

## Build and publish

From a source checkout after [host setup](README.md#host-development):

```sh
make gate
python3 firmware/scripts/release.py mi360 --version mi360-v0.1.0 --output /path/to/releases
python3 firmware/scripts/release.py reolink-e1-zoom --version reolink-e1-zoom-v0.1.0 --output /path/to/releases
python3 firmware/scripts/release.py ds-kd8003-ime1 --version ds-kd8003-ime1-v0.1.0 --output /path/to/releases
```

The release command builds the native application and static helpers, fetches the hash-pinned Dropbear
source (or accepts `--dropbear-source FILE`), and writes a deterministic archive and SHA-256 file.
Library interface manifests under `abi/` generate build-only ELF metadata; SDK interface declarations
live with each camera. No vendor library files or header downloads are involved.

`.github/workflows/firmware.yml` runs the host gate and the native builds on source changes.
`.github/workflows/firmware-release.yml` runs the gate and builds the selected bundle for a pushed
`MODEL-vMAJOR.MINOR.PATCH` tag (`mi360`, `reolink-e1-zoom` or `ds-kd8003-ime1`). It attaches all assets to a
draft, then publishes the release. A `-SUFFIX` tag produces a prerelease. Releases are model-specific,
so they do not set a repository-wide latest release. Manual runs build preview artifacts only.

### Publish a release

`MODEL` is `mi360`, `reolink-e1-zoom` or `ds-kd8003-ime1`. `PREV` is that model's latest tag
(`git tag --list 'MODEL-v*' --sort=-v:refname | head -1`), `NEW` the next version (`MODEL-vX.Y.Z`, or
`MODEL-vX.Y.Z-SUFFIX` for a prerelease).

1. Merge to `main` and wait for a green `firmware.yml` run.
2. Check that the model changed: `git log --oneline --first-parent PREV..main -- firmware/cameras/MODEL
   firmware/crates firmware/abi firmware/scripts` is not empty.
3. Tag the merge commit and push the tag:

   ```sh
   git tag -a NEW MERGE_COMMIT -m NEW
   git push origin NEW
   ```

4. Wait for the release workflow (`gh run watch`). It publishes the release with the archive and its
   `.sha256`. If it failed after creating the draft, delete the draft (`gh release delete NEW`) before
   rerunning; never move a pushed tag.
5. Add a short change list above the workflow's text, one line per user-visible change with its PR number:

   ```sh
   { printf '## Changes since PREV\n\n- CHANGE (#PR)\n\n'; gh release view NEW --json body -q .body; } > NOTES.md
   gh release edit NEW --notes-file NOTES.md
   ```

   Find the PRs with `git log --first-parent --format='%s%n%b' PREV..NEW -- PATHS` (paths from step 2).
   Keep `NOTES.md` out of the repository.

Compilation/host checks do not qualify hardware. Keep image preservation checks and on-camera
installation/media/control evidence separate from the release's model/source/build identity.

## DS-KD8003-IME1

Only the original non-B panel on stock V2.2.62 build 230204 is supported. The bundle has an installer
instead of an image builder: `python3 install.py` reads the panel's own firmware, so no stock download is
needed. Follow its `INSTALL.md`.
