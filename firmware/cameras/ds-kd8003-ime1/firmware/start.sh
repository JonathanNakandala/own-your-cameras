#!/bin/sh
# Boot supervisor for ds-kd8003-ime1-cam, run by the firmware's startup hook after the vendor application
# has started. Lives on the persistent config partition next to the payload; every path below is
# relative to this script's directory, so the hook's owner directory name does not matter.
#
#   native.enabled   opt-in flag: without it this script exits and stock keeps running
#   native.boots     boots since the last healthy run; 2 unhealthy boots in a row => stock
#   native.ok        written by ds-kd8003-ime1-cam next to cam.conf after it ran healthily (resets the counter)
#   native.fallback  appended when the counter trips; remove it and native.boots to re-arm
#   native/*.lzma    payload (ds-kd8003-ime1-cam, libraries, clips.tar), unpacked into RAM at boot
#   cam.conf         configuration; pins.conf PIN table; keypad.key 16-byte bootstrap key (0600)
#   ssh/authorized_keys  who may log in; ssh/host_ed25519 is generated here on the first native boot
#
# Rollback: delete native.enabled and reboot, or let two failed boots trip the fallback (after a
# healthy run that is three power cuts in a row, each inside the first two minutes of a boot).
# Recovery of a dead application is a reboot: once ds-kd8003-ime1-cam exits the watchdog is no longer fed and
# the media drivers are left for a reset, so this script reboots instead of restarting in place.
set -u
umask 077
# No dirname applet on this BusyBox: take the directory from $0 (the hook calls us by full path).
base=${0%/*}
[ "$base" = "$0" ] && base=.
base=$(cd "$base" && pwd)
runtime=${CAM_RUNTIME:-/home/app/custom/native}
[ -f "$base/native.enabled" ] && [ ! -e "$base/disabled" ] || exit 0
mkdir -p "$runtime" || exit 1
ulimit -c 0
# The hook inherits a 64 KiB file-size limit; root may raise it for the unpacked payload and logs.
ulimit -f unlimited 2>/dev/null || ulimit -f 65536
exec >>"$runtime/supervisor.log" 2>&1
echo "$(cat /proc/uptime) supervisor start in $base"

# This BusyBox shell has $((arith)) disabled; the counter only needs to reach 3.
boots=$(cat "$base/native.boots" 2>/dev/null || echo 0)
if [ -f "$base/native.ok" ]; then boots=0; fi
rm -f "$base/native.ok"
case "$boots" in 0) boots=1 ;; 1) boots=2 ;; *) boots=3 ;; esac
echo "$boots" > "$base/native.boots"
if [ "$boots" -gt 2 ]; then
    echo "fallback: $boots unhealthy boots, leaving stock running"
    echo "$(cat /proc/uptime) boots=$boots" >> "$base/native.fallback"
    exit 0
fi

# The config partition only ever holds compressed files. This BusyBox has unlzma and tar, no gunzip.
for f in "$base"/native/*.lzma; do
    [ -f "$f" ] || continue
    name=${f##*/}
    name=${name%.lzma}
    if ! unlzma -c "$f" > "$runtime/$name"; then
        echo "unpack $name failed"
        exit 1
    fi
done
if [ -f "$runtime/clips.tar" ]; then
    mkdir -p "$runtime/clips"
    tar -x -f "$runtime/clips.tar" -C "$runtime/clips" || { echo "clips unpack failed"; exit 1; }
    rm -f "$runtime/clips.tar"
fi
# unlzma -c creates the files with our umask (0600); the two programs need the execute bit.
chmod 755 "$runtime/ds-kd8003-ime1-cam"
[ -f "$runtime/dropbearmulti" ] && chmod 755 "$runtime/dropbearmulti"
# The stock image ships the ISP access library; the bridge loads it from the runtime directory.
if [ ! -e "$runtime/libe3_isp_access.so" ]; then
    for p in /home/app/lib/libe3_isp_access.so /lib/libe3_isp_access.so /usr/lib/libe3_isp_access.so; do
        [ -f "$p" ] && { ln -s "$p" "$runtime/libe3_isp_access.so"; break; }
    done
fi

# Maintenance SSH: our key-only Dropbear replaces the vendor daemon (password login, restricted shell).
if [ -x "$runtime/dropbearmulti" ] && [ -s "$base/ssh/authorized_keys" ]; then
    # The host key first: without it Dropbear cannot start, so the vendor daemon is left alone.
    [ -s "$base/ssh/host_ed25519" ] || "$runtime/dropbearmulti" dropbearkey -t ed25519 -f "$base/ssh/host_ed25519" >/dev/null 2>>"$runtime/dropbear.log"
    if [ -s "$base/ssh/host_ed25519" ]; then
        chmod 600 "$base/ssh/host_ed25519" "$base/ssh/authorized_keys"
        # The vendor daemon runs as "sshd" and holds port 22; root's shell in /etc/passwd is already /bin/sh.
        for proc in /proc/[0-9]*; do
            [ "$(cat "$proc/comm" 2>/dev/null)" = sshd ] && kill "${proc#/proc/}" 2>/dev/null
        done
        # Built without syslog: it logs to stderr by itself (no -E flag in this build); it daemonizes on success.
        if "$runtime/dropbearmulti" dropbear -r "$base/ssh/host_ed25519" -D "$base/ssh" -s -p 22 2>>"$runtime/dropbear.log"; then
            echo "$(cat /proc/uptime) dropbear started"
        else
            echo "$(cat /proc/uptime) dropbear failed to start, see dropbear.log"
        fi
    else
        echo "$(cat /proc/uptime) no Dropbear host key: vendor SSH left as it was"
    fi
else
    echo "no dropbear payload or authorized_keys: vendor SSH left as it was"
fi

# The hook runs us about 20 s into the boot, before the vendor application has applied the
# configured address (the image boots on the factory default below); ds-kd8003-ime1-cam stops that application
# as soon as it starts. Wait for the real address, remember it, and put it back whenever it is gone.
cidr=''
gw=''
for attempt in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30; do
    line=$(ip -4 addr show dev eth0 2>/dev/null | grep 'inet ')
    set -- $line
    case "${2:-}" in
        ''|192.0.0.64/*) sleep 3 ;;
        *) cidr=$2; set -- $(ip -4 route show default 2>/dev/null); gw=${3:-}; [ -n "$gw" ] && break; sleep 3 ;;
    esac
done
if [ -n "$cidr" ]; then
    echo "$(cat /proc/uptime) network $cidr via $gw"
else
    echo "$(cat /proc/uptime) no configured address appeared, starting anyway"
fi

echo "$(cat /proc/uptime) start ds-kd8003-ime1-cam (boot $boots)"
LD_LIBRARY_PATH=$runtime "$runtime/ds-kd8003-ime1-cam" --run "$runtime" "$base/cam.conf" >>"$runtime/cam.log" 2>&1 &
pid=$!
while kill -0 "$pid" 2>/dev/null; do
    if [ -n "$cidr" ] && ! ip -4 addr show dev eth0 2>/dev/null | grep -q "inet $cidr "; then
        ip addr add "$cidr" dev eth0 2>/dev/null
        [ -n "$gw" ] && ip route add default via "$gw" dev eth0 2>/dev/null
        echo "$(cat /proc/uptime) restored $cidr via $gw"
    fi
    sleep 5
done
wait "$pid"
code=$?
echo "$(cat /proc/uptime) ds-kd8003-ime1-cam exited $code; rebooting"
sync
reboot
sleep 30
exit 1
