# DS-KH9510-WTE1: root SSH

[DS-KH9510-WTE1 overview](README.md)

**Scope:** DS-KH9510-WTE1, R1 / rk312x, Android 5.1.1,
**V2.2.28 build 230613**. Administrator credentials and a physical panel tap are required.

The original helper confirmed root execution and persistent SSH on this build.
The smaller helper here is newly written: built and inspected offline, **not yet
run on a panel**. Other builds and hardware variants are untested.

## 1. Build your helper

On your computer, install Python 3, a JDK, Docker, and the
[Android SDK command-line tools](https://developer.android.com/tools).
Install API 22 and Build Tools 28.0.3 through the SDK manager. Set `ANDROID_SDK_ROOT`
to your SDK directory. Run from the repository root:

```sh
sdkmanager 'platforms;android-22' 'build-tools;28.0.3'
ssh-keygen -t rsa -b 3072 -f ~/.ssh/ds_kh9510_wte1

docker build -f tools/ds-kh9510-wte1/Dockerfile.aapt -t ds-kh9510-wte1-aapt .
python3 tools/ds-kh9510-wte1/build.py \
  --android-jar "$ANDROID_SDK_ROOT/platforms/android-22/android.jar" \
  --dx-jar "$ANDROID_SDK_ROOT/build-tools/28.0.3/lib/dx.jar" \
  --apksigner-jar "$ANDROID_SDK_ROOT/build-tools/28.0.3/lib/apksigner.jar" \
  --public-key ~/.ssh/ds_kh9510_wte1.pub
```

Expected: `build/ds-kh9510-wte1/ds-kh9510-wte1-root-helper.apk`, signed with a new local
development key. An existing RSA public key also works; never supply the private key.
The APK embeds your public key. Keep `build/` and the APK private. The helper has no
network listener, internet permission, reporting, or automatic startup.

## 2. Install it

This device's HTTP ports were closed. We used **ISAPI over the vendor SDK on port
8000**, not HTTP curl requests. Download the Linux 64-bit **Device Network SDK**
from [Hikvision](https://www.hikvision.com/en/support/download/sdk/).
Set `SDK_LIB_DIR` to the absolute directory containing `libhcnetsdk.so`, its bundled
libraries, and `HCNetSDKCom/`. No vendor SDK binaries are included here.

```sh
docker build --platform linux/amd64 \
  -f tools/ds-kh9510-wte1/Dockerfile.sdk -t ds-kh9510-wte1-sdk .

# Define a local convenience function. Each request prompts for the admin password.
station_request() {
  docker run --rm -it --platform linux/amd64 \
    -v "$SDK_LIB_DIR:/sdk:ro" -v "$PWD:/work:ro" ds-kh9510-wte1-sdk \
    --host CAMERA_HOST "$@"
}
station_request --method GET --path /ISAPI/System/deviceInfo
station_request --method GET --path '/ISAPI/VideoIntercom/Applications/1?format=json'
station_request --method GET --path '/ISAPI/VideoIntercom/Applications/2?format=json'
```

Replace `CAMERA_HOST` in the function with your device's address. Confirm the model
and build. Inspect the two application slots above and find an **empty application slot**; below, replace `SLOT` with its number.
Do not replace an installed app. Device responses may contain private identifiers.
The rewritten SDK wrapper is offline checked; a successful SDK call alone does not
mean the device accepted the request. Inspect its response/status too.

Prepare a directory containing **only** the APK, plus the install requests:

```sh
mkdir -p build/ds-kh9510-wte1/serve
cp build/ds-kh9510-wte1/ds-kh9510-wte1-root-helper.apk build/ds-kh9510-wte1/serve/
cat > build/ds-kh9510-wte1/install.json <<'JSON'
{"AppInstallCfg":{"id":SLOT,"downloadUrl":"http://COMPUTER_HOST:8768/ds-kh9510-wte1-root-helper.apk","method":"immediate"}}
JSON
cat > build/ds-kh9510-wte1/download.json <<'JSON'
{"AppManage":{"id":SLOT,"manualCmd":"download"}}
JSON
```

Start the server in a second terminal:

```sh
python3 -m http.server 8768 --directory build/ds-kh9510-wte1/serve
```

Replace `SLOT` in both files with the same integer (without quotes) and
`COMPUTER_HOST` with your computer's address reachable from the device. Then:

```sh
station_request --method PUT \
  --path '/ISAPI/VideoIntercom/Applications/SLOT/installConfig?format=json' \
  --body-file /work/build/ds-kh9510-wte1/install.json
station_request --method PUT \
  --path '/ISAPI/VideoIntercom/Applications/SLOT/manage?format=json' \
  --body-file /work/build/ds-kh9510-wte1/download.json
station_request --method GET --path '/ISAPI/VideoIntercom/Applications/1?format=json'
station_request --method GET --path '/ISAPI/VideoIntercom/Applications/2?format=json'
```

Replace `SLOT` in the request paths too. Wait for **DS-KH9510-WTE1 Root Helper** to appear
installed; recheck all slots because update metadata can settle in a different slot.
Stop the HTTP server with Ctrl-C after installation.

## 3. Open it on the panel

1. Physically tap **DS-KH9510-WTE1 Root Helper** in the application list.
2. Tap **Check root (id)**. Expected: `uid=0(root)` and `Exit: 0`.
3. Tap **Install bundled SSH public key**, then confirm the write.
4. Expected: `Key installed; /system restored read-only` and `Exit: 0`.

The installer preserves existing keys and makes a one-time `.before-ds-kh9510-wte1-helper`
backup when an authorized_keys file already exists. It checks the observed ext4
`/system` mount and block write protection before writing. If checks fail, stop and
inspect your build. A timeout means the outcome is unknown; do not retry blindly.

## 4. Enable SSH and connect

If SSH is disabled, use the same authenticated SDK transport:

```sh
cat > build/ds-kh9510-wte1/ssh.xml <<'XML'
<SSH><enabled>true</enabled></SSH>
XML
station_request --method PUT --path /ISAPI/System/Network/ssh \
  --body-file /work/build/ds-kh9510-wte1/ssh.xml

ssh -o IdentitiesOnly=yes -i ~/.ssh/ds_kh9510_wte1 root@CAMERA_HOST
```

Verify the host key through your own trusted channel. At the prompt, run `id`;
expected: `uid=0(root)`. Exit with `exit`. **A PTY is required**: do not use `ssh -T`
or append a remote command. For piped commands:

```sh
printf 'id\nexit\n' | ssh -tt -o IdentitiesOnly=yes \
  -i ~/.ssh/ds_kh9510_wte1 root@CAMERA_HOST
```

After a successful independent SSH login, remove the helper through the panel's
application management. The installed SSH key remains. Password authentication
also remains enabled. Keys survived reboots in the original work; firmware updates
can replace `/system`. No firmware flashing is needed for this procedure.

## How it works

An Android app connects to the abstract Unix socket `@cliserver`. On the tested
build, the service runs supplied commands with root `popen()` and does not check
the caller. Root redirects results into files precreated by the app.

The helper writes your key to `/system/etc/dropbear/.ssh/authorized_keys` with:

```text
command="OTA=y ENV=/dev/null TMPDIR=/data/local/tmp PATH=/system/bin:/system/xbin:/sbin:/usr/bin:/bin /system/bin/sh" ssh-rsa YOUR_PUBLIC_KEY
```

The forced command starts Android mksh while skipping the startup file that launches
the restricted `psh`. The installer briefly clears software write protection on
`/dev/block/mmcblk0p12`, remounts `/system` writable, then restores both read-only
settings. Do not apply this partition layout to another build without inspection.
