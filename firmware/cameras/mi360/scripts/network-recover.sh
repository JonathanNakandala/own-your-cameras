#!/bin/sh
# Independent, explicitly invoked Wi-Fi recovery. No flash, NVRAM or boot changes.
set -eu
PATH=/mnt/usr/bin:/bin:/sbin:/usr/bin:/usr/sbin
LD_LIBRARY_PATH=/lib:/mnt/usr/lib
export PATH LD_LIBRARY_PATH
umask 077
base=/mnt/data/mi360-network
run=/tmp/mi360-network
request=${1:-manual}
case "$request" in ''|*[!a-z0-9]*) exit 2 ;; esac
[ "${#request}" -le 64 ] || exit 2
[ -d "$base" ] && [ ! -L "$base" ]
[ -f "$base/wpa.conf" ] && [ ! -L "$base/wpa.conf" ]
[ -x "$base/dhcp.sh" ] && [ -x "$base/wpa-event.sh" ] && [ -x "$base/ap.sh" ]
[ ! -L "$run" ]
mkdir -p "$run"
chmod 700 "$run"
exec 9>"$run/recover-v2.lock"
flock -n 9 || exit 1
rm -f "$run/last-success"
# Let the SSH launch acknowledgement leave before changing the interface.
sleep 1

# Copy the real, previously working configuration; never source credential text.
# Give this session its own control socket, independent of the Xiaomi helpers.
# A network without its own bgscan gets one: wpa_supplicant otherwise keeps the
# first access point it joined for good, even after a much stronger one carrying
# the same SSID appears (2026-11-08: -77 dBm at 1 Mbit/s beside one at -55 dBm,
# and talk sessions stalled). Below -65 dBm it scans every 30 s, else every 300 s.
mkdir -p "$run/ctrl"
{ printf 'ctrl_interface=%s/ctrl\nupdate_config=0\n' "$run"
  sed '/^[[:space:]]*ctrl_interface=/d; /^[[:space:]]*update_config=/d' "$base/wpa.conf" |
      if grep -q '^[[:space:]]*bgscan=' "$base/wpa.conf"; then cat; else
          sed '/^[[:space:]]*network={/a\
    bgscan="simple:30:-65:300"'
      fi
} >"$run/wpa.conf"
chmod 600 "$run/wpa.conf"

# Stop only the pairing owners and the stock network repair loop. Media and the
# hardware watchdog remain untouched. This takeover lasts until the next reboot.
connected=0
fallback() {
    status=$?
    trap - 0
    if [ "$connected" = 0 ]; then
        echo 'network recovery failed; restoring the camera hotspot'
        (exec 9>&-; exec /bin/sh "$base/ap.sh") >"$run/fallback.log" 2>&1 || :
    fi
    exit "$status"
}
trap fallback 0
trap 'exit 1' HUP INT TERM
# A custom-image boot intentionally leaves every vendor unit unactivated.
# Request shutdown when the supervisor knows them, then verify actual owners;
# an absent/unactivated unit is not evidence of a live pairing process.
perpctl -b /tmp/perp d miio_client_helper miio_client >/dev/null 2>&1 || :
sleep 1
for process in /proc/[0-9]*/cmdline; do
    command=$(tr '\000' ' ' <"$process" 2>/dev/null) || continue
    case "$command" in
        'miio_client '*|'/mnt/usr/bin/miio_client '*|'/bin/sh /mnt/data/bin/miio_client_helper_nomqtt.sh '*|'/bin/sh /mnt/usr/bin/miio_client_helper_nomqtt.sh '*)
            echo 'network recovery: vendor pairing owner did not stop'
            exit 1
            ;;
        '/bin/sh /mnt/data/bin/network_governor.sh '*|'/bin/sh /mnt/usr/bin/network_governor.sh '*)
            pid=${process#/proc/}; pid=${pid%/cmdline}
            kill -TERM "$pid" 2>/dev/null || :
            ;;
    esac
done
/bin/busybox killall -q -TERM wpa_cli udhcpc wpa_supplicant hostapd udhcpd || :
# Wait for old owners to leave before creating the new driver session.
tries=0
while /bin/busybox pidof wpa_cli udhcpc wpa_supplicant hostapd udhcpd >/dev/null; do
    tries=$((tries + 1))
    [ "$tries" -lt 5 ] || { echo 'network recovery: old owner did not exit'; exit 1; }
    sleep 1
done
ifconfig wlan0 down
ifconfig wlan0 up
iwconfig wlan0 mode Managed
# Old ash can leak its temporary saved descriptor when `9>&-` is attached to a
# command. Permanently close it in a subshell before exec, so daemons cannot keep
# the recovery lock after this script exits.
(exec 9>&-; exec /usr/bin/wpa_supplicant -Dnl80211 -iwlan0 -c "$run/wpa.conf" -B -P "$run/wpa.pid") \
    >"$run/wpa.log" 2>&1
(exec 9>&-; exec /sbin/udhcpc -i wlan0 -b -t 9 -T 2 -A 10 -p "$run/udhcpc.pid" \
    -s "$base/dhcp.sh" -x hostname:chuangmi_camera_046a01) >"$run/dhcp.log" 2>&1
(exec 9>&-; exec /usr/bin/wpa_cli -p "$run/ctrl" -i wlan0 -B -P "$run/event.pid" \
    -a "$base/wpa-event.sh") >"$run/event.log" 2>&1
tries=0
while [ "$tries" -lt 60 ]; do
    state=$(/usr/bin/wpa_cli -p "$run/ctrl" -i wlan0 status 9>&- 2>/dev/null) || state=
    case "$state" in
        *wpa_state=COMPLETED*)
            if printf '%s\n' "$state" | grep -q '^ip_address='; then
                connected=1
                printf '%s\n' "$request" >"$run/last-success"
                echo 'network recovery: associated with an IPv4 lease; credentials retained'
                exit 0
            fi
            ;;
    esac
    tries=$((tries + 1))
    sleep 1
done
exit 1
