# Qualified hardware boundary

Only MJSXJ10CM / T31 / SC2336 on the pinned 4.3.4_0376 stock kernel and SDK is supported. Other Mi 360
models, sensors and kernels need separate qualification even if device nodes have the same names.
The [stock component lock](../firmware/stock.lock.json) identifies the accepted ROOTFS, kernel, SDK,
modules and tuning files. The image recipe keeps those components unchanged.

## Ownership and shutdown

`src/media.rs` owns one IMP system and serializes its C adapters in `sdk/`. The C interfaces assert
actual O32 structure sizes/offsets; host sanitizer fixtures test lifecycle and bounds without loading
proprietary libraries. Failed acquisition/cleanup retains uncertain ownership and requests a clean
boot. It never hands a partially initialized SDK to a second owner.

`src/board.rs` owns motor/GPIO policy; `src/board/linux.rs` owns device/sysfs/ioctl I/O. A separate
hardware worker prevents ordinary board operations from blocking the media schedule. SDK calls and
kernel ioctls can still block; the independent watchdog is the final escalation boundary.

The motor uses qualified normal travel, pan 0–2280 and tilt 0–650 raw steps. Those counters are not
encoders. Startup and manual Home perform a finite physical reference seek and validated center
return. Moving the head by hand while powered requires Home again. Rotating the camera body changes
the world view and cannot be corrected by head homing. Gear play means raw counter equality alone
does not guarantee an identical view; presets retain each axis's last approach direction.

The timer repair and atomic retarget path in `src/motor_clock.rs` accesses TCU channel 2 through
`/dev/mem`. Its qualified assumptions include one CPU, hard IRQ delivery and the exact stock driver.
It preserves watchdog/channel 0 state. Do not run it against an unqualified kernel; matching an SDK
version string or sensor name is insufficient.

The qualified day/night bridge pulses GPIO 51 or 54 for 500 ms, then lowers both; illumination uses
GPIO 49. GPIO readback cannot reveal the mechanical filter position after the pulse. The speaker
amplifier has its own `/dev/speaker` lifetime, separate from audio buffer delivery.

## Image pointing

A valid optional root-owned 0600 `/mnt/data/mi360/position3d.json` enables image-point requests. Missing
calibration disables them; there are no guessed lens coefficients. The immediate directory must be
root-owned 0700. Symlinks, hard links, duplicate/unknown fields and files over 4096 bytes are refused.

The flat JSON schema contains `version: 1`, `pan_a`, `pan_b`, `tilt_a`, `tilt_b`,
`pan_right_grows`, `tilt_up_grows` and optional `display_delay_ms`. Each model is `a*d + b*d^3`
in raw steps, where offsets are frame widths. The direction fields are booleans describing increasing
counters with logical Mirror/Flip off. Coefficients must be finite, bounded and strictly increasing
on the image domain. See `Calibration` in `src/pointing.rs` for the exact schema and validation.

Measure your own unit using recorded real frames and same-direction motor movements, fit pan and tilt
separately, and validate against independent image pairs before enabling this feature. Do not infer
coefficients from the Reolink lens or reuse household recordings as public fixtures. Display delay
also depends on the viewing client. Open-loop pointing is approximate, not angular/encoder feedback.

## RTSP limits

At most six RTSP connections with 96 KiB worker stacks; 1 MiB access units, 2 MiB retained video and four
queued frames.

## Audio and recovery limits

The microphone is captured at 48 kHz (IMP AI device 1, 40 ms frames of 1,920 samples) and encoded to
Opus (libopus 1.5.2, 64 kbit/s); the speaker plays 16 kHz (IMP AO device 0, 640-sample blocks), which
covers its output: within −20 dB of 1 kHz from ~500 Hz to 6.4 kHz, −32 dB at 8 kHz
([measurements](../../../../docs/shared/camera-audio-response.md)). The two devices
run at different rates without affecting each other. No SDK effects (NS, AGC, AEC) are used.

The AO device restarts, and loses audio, whenever its stream stops, so the daemon feeds it silence
between talk bursts, paced by the clock. The speaker amplifier switches on when talk starts and off
one second after it ends; audio starts 60 ms after switch-on so the switching transient never lands
on speech. A burst's losses are logged as `speaker: burst ended played_ms=… expired_ms=…
overflow_ms=… full=…`. Talkback is Opus on track 2, up to four talkers mixed on the camera, the
same transport as the Reolink E1 Zoom. Driver delivery does not establish speaker loudness or
acoustic echo cancellation; those remain attended acceptance checks. Host tests never open a
microphone or produce sound.

The camera has a single shared boot environment. A userspace boot-attempt budget can recover from a
daemon failure only while the kernel and SSH/network recovery can boot. It cannot repair a corrupt
boot environment or guarantee survival of an interrupted flash write. Keep the original full dump
and established external-programmer access.
