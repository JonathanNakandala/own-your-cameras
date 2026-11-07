# Root access and persistent SSH on G3/G5

Root is the camera's administrator shell. It lets you inspect files and run your
own programs. Getting a shell and keeping it after a reboot are separate jobs.

**Applies to:** DS-2CD2T87G2-L G3 and DS-2CD2386G2-ISU/SL G5. The DS-KD8003-IME1
has a different [access method](../devices/ds-kd8003-ime1/README.md#get-a-root-shell).
Examples use `CAMERA_HOST` for your camera's address. Keep credentials and backups
on your own machine, outside this repository.

## 1. Get the first shell

On G3 V5.5.154 build 200928 and G5 V5.5.113, CVE-2021-36260 was used to run commands
as root. The request is an unauthenticated `PUT /SDK/webLanguage`, with:

```text
X-Requested-With: XMLHttpRequest
Content-Type: application/x-www-form-urlencoded; charset=UTF-8

<?xml version="1.0" encoding="UTF-8"?><language>$(COMMAND)</language>
```

The observed command limit was **22 bytes**. Longer commands failed or were
truncated. The web process runs from `/home`; `webLib` is HTTP-served content.
Never put keys, passwords or settings there.

The recorded bootstrap created a temporary UID-0 shell account and a temporary
Dropbear server, then restored the administrator's password and changed its login
shell from `/bin/psh` to `/bin/sh`. The temporary account, server and web output
files were removed afterwards. This repository does not include that exploit
runner. See the [build results and advisory](cve-2021-36260.md).

Once a full shell is available, check:

```sh
id
uname -m
```

Expected: `uid=0` and `armv7l`. On these cameras the Linux login is `admin`, not
`root`. The administrator account's home is `/`, so its SSH keys belong in
`/.ssh/authorized_keys`. Do not replace `/etc/passwd` with a sample: it contains
the real authentication data.

## 2. Install a separate key-only SSH service

The **loader** starts the **worker**; the worker restores keys and maintains SSH.

The tested approach kept the vendor SSH service separate and ran owner access
on port 2222. Use an **ECDSA P-256** client key; the inspected Dropbear did not
support Ed25519:

```sh
# On your computer; choose a private location for this key.
ssh-keygen -t ecdsa -b 256 -f CAMERA_KEY
```

From the existing root session, the required pieces are:

| File / action | Purpose |
|---|---|
| Persistent `authorized_keys` containing your public key | Restore `/.ssh/authorized_keys` at startup |
| Persistent copy of `/etc/dropbear/dropbear_ecdsa_host_key` | Keep the server identity stable across reboots |
| Persistent fallback copy of `/usr/sbin/dropbear` | Keep an SSH binary available after an update |
| Startup worker | Restore admin's shell and key file after vendor initialization |
| Source-restricted firewall rule for port 2222 | Limit who can reach the maintenance service |

The server invocation used these options, with paths replaced by your own:

```sh
/usr/sbin/dropbear -s -p 2222 -r HOST_KEY_FILE -P PID_FILE -K 30 -I 1800
```

`-s` disables password login. The worker rechecked the shell, keys, firewall rule
and server every 20 seconds because vendor startup could reset them. This is a
description of the tested mechanism; a portable installer is not included.

Pin and compare the server fingerprint before using the new connection. Then:

```sh
# On your computer; the first connection asks you to verify the host key.
ssh -T -p 2222 -i CAMERA_KEY admin@CAMERA_HOST
# In the camera shell:
id
exit
```

The inspected SSH server rejected remote exec requests. Use a shell with commands
on stdin instead of `ssh CAMERA_HOST command`:

```sh
printf 'id\nexit\n' | ssh -T -p 2222 -i CAMERA_KEY admin@CAMERA_HOST
```

## 3. Make startup survive an update

- **G3:** `/dav/initrun.sh` starts the application. The tested update replaced it.
  Restore the owner hook after upload and **before reboot**. See the [G3 sequence](../devices/ds-2cd2t87g2-l/README.md#upgrade-without-losing-access).
- **G5:** V5.7.23 calls `/dav/custom_initrun.sh` itself. That hook and the access
  files survived the tested upgrade and same-version reinstall. See the [G5 sequence](../devices/ds-2cd2386g2-isu-sl/README.md#upgrade-without-losing-access).

Keep a native configuration export, its restore password, and a readable copy of
important settings before upgrading. Reading a backup successfully does not prove
that an old database can be restored into a new firmware schema.

## If access disappears

If the camera's web interface still works, use its supported SSH toggle to check
whether the vendor service is available. That may provide only the restricted
shell; it does not automatically restore full root. A reboot clears RAM changes.

If a startup hook is missing and no full shell remains, these notes do not provide
a tested recovery procedure for patched firmware. Preserve your current session
until the hook has been checked. Factory reset can remove access and settings;
serial-console recovery was not validated in this work.

To remove custom access, stop the worker **and** its SSH server, remove its firewall
rule and keys, and restore the original startup file. A disable flag alone does
not stop an already running server. Removal commands depend on the paths and
process tracking used by your implementation.
