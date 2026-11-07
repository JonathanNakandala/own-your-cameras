# Recovery

## Fallback to stock

[`start.sh`](../firmware/start.sh) counts boots in `native.boots`; the application writes `native.ok`
after two minutes of streaming and the next boot resets the count. Two boots in a row without that
mark leave the vendor application running and append a line to `native.fallback`. After a healthy
run that takes three power cuts in a row, each within the first two minutes of a boot: the first two
count as unhealthy, the third stays on stock. Re-arm with
`rm native.boots native.fallback` next to `start.sh`.

## When the application stops

Once the application exits for any reason the supervisor reboots the device: the media drivers are
left for a reset, and nothing feeds the watchdog any more. The watchdog is fed only while the
application's main loop beats: a loop stalled for 6 s stops the feeding and the hardware watchdog resets the board.

If the application dies while the strike is energized (a panic, a fault in the C bridge, SIGKILL),
the strike stays on until that watchdog reset, a few seconds. A strike release that keeps failing
makes the application exit on purpose for the same reset.

## Lost SSH key

A lost key is not fatal: cutting power three times as above returns the stock firmware with its own
maintenance paths. Root's shell in `/etc/passwd` is already `/bin/sh`; the restriction was the
vendor daemon's own.

## Address

The hook starts the supervisor before the vendor application applies the configured address (the
image boots on the factory default), so the supervisor waits up to 90 s for that address, remembers
it and puts it back with `ip` whenever it disappears after the vendor processes are stopped. If it
never appears the application starts anyway and the device stays on the factory address.
