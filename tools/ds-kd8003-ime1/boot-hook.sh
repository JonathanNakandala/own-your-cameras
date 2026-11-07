# OWNER_CUSTOM_BOOT_V1 -- local customization hook, appended after vendor startup.
# All default writes are volatile. User code requires an explicit persistent flag.
(
    umask 077
    /bin/mkdir -p /home/app/custom || exit 0
    /bin/echo 'ds-kd8003-ime1-custom-boot-v1' > /home/app/custom/boot-marker
    if [ -f /home/config/custom/enabled ] &&
       [ ! -e /home/config/custom/disabled ] &&
       [ -f /home/config/custom/start.sh ]; then
        ulimit -c 0
        ulimit -f 128
        exec /bin/sh /home/config/custom/start.sh </dev/null > /home/app/custom/start.log 2>&1
    fi
) </dev/null >/dev/null 2>&1 &
