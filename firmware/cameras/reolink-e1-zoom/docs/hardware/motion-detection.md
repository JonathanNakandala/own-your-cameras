# Motion detection (not implemented)

What the stock firmware does for motion, what that means for `cam`, and why `cam` does not report motion yet.
Evidence tags as in [README.md](README.md#evidence-tags): everything below is **[D]** (read from the stock 4417
firmware's files and disassembly) unless marked **[I]** (inferred). Nothing here was run on a camera.

## What the vendor stack has

- The stock `device` carries a Novatek **`vendor_md`** layer, statically linked (it is not in `libhdal.so`):
  `vendor_md_init`, `vendor_md_set`, `vendor_md_trigger`, `vendor_md_uninit` with the messages
  "Open kdrv_md fail", "md opencfg fail", "md set VENDOR_MD_PARAM fail" and "ai should be init before trigger".
  `device` calls them through `Na_md_init`, `Na_md_process` and `Na_md_uninit`.
- The kernel side is `kdrv_md.ko` (`kdrv_md_open`, `_set`, `_get`, `_trigger`, `_set_param`, `_get_param`,
  `_set_isr_cb`), a hardware motion-detection engine.
- It is driven from user space **per frame**: `Na_md_process` sets parameters then triggers the engine, and the
  engine needs the AI unit initialised first [I: the engine works on frames handed to it, not on the sensor
  stream by itself].
- The vendor messages are `MSG_CFG_MD_GET/SET/NOTIFY` and `MSG_ENC_MD_STAT_MODIFY`; `device` also logs
  `md_on chnl:%d` / `md_off chnl:%d`. Person, vehicle, pet and face alerts are separate messages in `device`, and
  the NN libraries are present (`libprebuilt_pdcnn_ai2.so`, `libprebuilt_fdcnn_ai2.so`,
  `libprebuilt_cddcnn_ai2.so`) [I: these are what those alerts run on].

## The ISP's own motion items

`nvt_isp.ko` also exposes `ISPT_ITEM_MD_DATA` and `ISPT_ITEM_MD_STA` (`ispt_api_get_md_data`,
`isp_api_get_md_sta`). In `device` the ISP ioctl dispatcher (VMA `0x158600`..`0x158ca4`, one jump-table case per
request) issues them as type `'c'` requests with a 4-byte size field, like the `'q'` and `'e'` items of
[isp-iq.md](isp-iq.md#transport-and-safety):

| Request | Case | Failure message |
|---|---|---|
| `0xc0046330` | `0x15864c` | `ISP_IOC_GET_MD_DATA fail!` |
| `0xc0046332` | `0x1586bc` | `ISP_IOC_GET_MD_STA fail!` |
| `0xc0046331`, `0xc0046321`, `0x8004631f` | `0x158614`, `0x158684`, `0x1586f4` | not identified |

The payload layouts (the size field encodes a pointer) and what the two items mean (a block map and a status
word of the ISP's own motion statistics, or the result of the `kdrv_md` run) were not read. These are the
ISP's items, which the vendor uses with the image pipeline; whether they report anything without `device`
configuring `kdrv_md` first is unknown.

## Why `cam` reports no motion

Using the hardware would mean re-implementing `vendor_md` from the disassembly (the engine's parameter and
result structures, the AI-unit setup it depends on, and handing it frames by physical address), then proving it on
a camera that resets on a wrong media call (the watchdog budgets in [../architecture.md](../architecture.md#threads-and-the-watchdog)).
That is days of work for an event Frigate does not need (it detects on the picture itself).

## Options if motion events are wanted

1. **From the encoder's output** (no new hardware use). The RTSP hub already sees every access unit. A P frame
   much larger than the recent ones means the picture changed. The daemon knows its own causes of change (a
   pan/tilt/zoom move, an autofocus search, the IR-cut switching, a stream retune) and can ignore frames around
   them. Cheap and host-testable, but crude: lighting and exposure changes still trigger it, and it has no
   regions. Needs real footage to tune. [I]
2. **From the sub-stream pictures**: decode the hardware JPEG snapshot at 1-2 fps and difference small
   frames. Better selectivity, but a software JPEG decoder joins the dependencies and costs CPU on a single
   Cortex-A9. [I]
3. **Reverse `vendor_md`** and use `kdrv_md`. Best result, highest risk and cost.

Whichever source is used, the reporting side is small: an ONVIF `tns1:RuleEngine/CellMotionDetector/Motion`
event with an `IsMotion` property (the pull-point code already carries `tns1:Device/Trigger/DigitalInput`)
and an MQTT binary sensor.
