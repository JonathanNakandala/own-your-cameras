#!/system/bin/sh
set -eu
PATH=/system/bin:/system/xbin:/sbin:/usr/bin:/bin
export PATH
key=$1
partition=/dev/block/mmcblk0p12
keys=/system/etc/dropbear/.ssh/authorized_keys
# Fail closed on a different mount layout or an already-writable system.
busybox awk '$1=="/dev/block/mmcblk0p12" && $2=="/system" && $3=="ext4" && $4 ~ /(^|,)ro(,|$)/ {ok=1} END {exit !ok}' /proc/mounts
[ "$(busybox blockdev --getro "$partition")" = 1 ]
[ -s "$key" ]
[ ! -L /system/etc/dropbear/.ssh ]
[ ! -L "$keys" ]
restore() {
    failed=0
    busybox mount -o remount,ro "$partition" /system || failed=1
    busybox blockdev --setro "$partition" || failed=1
    if [ "$failed" != 0 ]; then
        echo 'ERROR: could not restore read-only state. Inspect before continuing.'
        return 1
    fi
}

trap restore EXIT
busybox blockdev --setrw "$partition"
busybox mount -o remount,rw "$partition" /system
busybox mkdir -p /system/etc/dropbear/.ssh
busybox chown 0:0 /system/etc/dropbear/.ssh
busybox chmod 700 /system/etc/dropbear/.ssh
# Preserve original bytes in a one-time backup on the device.
if [ -f "$keys" ] && [ ! -e "$keys.before-ds-kh9510-wte1-helper" ]; then
    busybox cp -p "$keys" "$keys.before-ds-kh9510-wte1-helper"
fi
line="command=\"OTA=y ENV=/dev/null TMPDIR=/data/local/tmp PATH=/system/bin:/system/xbin:/sbin:/usr/bin:/bin /system/bin/sh\" $(busybox cat "$key")"
if ! busybox grep -qxF "$line" "$keys" 2>/dev/null; then
    # Leading newline also handles an existing file without a trailing newline.
    printf '\n%s\n' "$line" >> "$keys"
fi
busybox chown 0:0 "$keys"
busybox chmod 600 "$keys"
busybox sync
restore
trap - EXIT
echo 'Key installed; /system restored read-only. Test SSH with a PTY.'
