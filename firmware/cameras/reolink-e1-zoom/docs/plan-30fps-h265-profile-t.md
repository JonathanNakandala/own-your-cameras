# Plan: 30 fps main stream, H.264/H.265 switching, ONVIF Profile T

Status: parts 1 and 3 implemented (2026-11-15), part 2 not started. Decisions taken are in "Decisions" at the end. Every [V] below was measured on the camera in RAM-only sessions
(`cam dump` with temporary experiment switches, since reverted); [D] is from the vendor firmware or the ONVIF
specification.

## Summary

| Part | Finding | Size of the work |
|---|---|---|
| 1. 30 fps main at 2592x1944 | Already works with 3D noise reduction on (the default): main 297 of 300 frames, sub at full rate even at 640x480@15 [V]. The sub stream does not need lowering. Only the graph without noise reduction starves the sub stream at 30 fps. | Small: new defaults, one graph fix, optional sub settings |
| 2. Profile T alongside S | Profile T v1.0 needs H.264 **or** H.265, and HTTPS is optional [D]. The real gaps are editable media profiles, metadata streaming, motion and tampering events, on-screen text, multicast and RTSP over HTTP. | Large: several independent pieces in the shared ONVIF and RTSP crates |
| 3. Switch H.264/H.265 | The encoder does H.265 at full size with noise reduction, gradual refresh and slices, at the same CPU [V]. The RTSP server, hub and ONVIF descriptions are H.264-only. | Medium: mostly `camera-transport` |

Suggested order: 1, then 3, then 2. Part 1 is mostly defaults. Part 3 gives the biggest picture gain per bit. Part 2
is the longest and benefits from 3 (Profile T's `SetVideoEncoderConfiguration` is the natural way to switch codec,
size and rate).

## 1. 30 fps main stream at full resolution

### What was measured [V]

10 s dumps, main 2592x1944@30 at 4096 kbit/s:

| Graph | Sub stream asked for | Main frames | Sub frames |
|---|---|---|---|
| plain (no 3DNR) | 640x480@10 | 298 / 300 | **1** / 100 |
| plain, pool of 8 blocks instead of 4 | 640x480@10 | `hd_common_mem_init` -34 (no memory) | — |
| plain | 640x360@5 | 298 | **1** / 50 |
| 3DNR | 640x480@10 | 297 | 99 / 100 |
| 3DNR | 640x360@5 | 297 | 50 / 50 |
| 3DNR | 640x480@15 | 297 | 148 / 150 |

In the plain graph every processor output draws from one pool of four 2592x1944 blocks; at 30 fps the main path
keeps them all in flight, and the processor drops the sub output when it asks for a buffer (`vprc` out 1:
`NEW 30, drop 19, wrn 10, PUSH 1`). The 3DNR graph already gives each output its own pool sized for it
([hardware/isp-iq.md](hardware/isp-iq.md#memory)), and there the sub stream is unaffected. Lowering the sub stream
does not help the plain graph and is not needed with 3DNR.

Also found: the default main size is 1920x1440, but 3DNR only runs with a 2592x1944 main stream, so the default
`isp_3dnr = ON` has no effect on a camera with default settings.

### Steps

1. **Check in production first, no code.** Over MQTT: `main_resolution` 2592x1944, `main_fps` 30,
   `main_bitrate` 8192. Verify in tinyCam over UDP, with `/proc/hdal/vprc/info` and `venc/info` for drops, the
   snapshot path (the dumps above build no JPEG path; production does), RTSP pacing (25 ms of each 33 ms frame) and
   Wi-Fi. Gradual refresh scales itself (`Gdr::per_second`: 122 rows / 30 fps = 5 rows a frame, GOP 240).
2. **New defaults:** `main_resolution` 2592x1944 (so 3DNR takes effect), `main_fps` 30, `main_bitrate` 8192.
   At 30 fps each frame gets half the bits of 15 fps, so the bitrate goes to the top of the range (stock ran
   5.2 Mbit/s at 20 fps). Only settings absent from a camera's file take the new defaults; a camera with saved
   values keeps them.
3. **Fix the plain graph:** per-output pools like the 3DNR graph (main-size blocks for the main output, small ones
   for sub and snapshot), so 30 fps also works with 3DNR off or a smaller main size. Size them inside the 47.5 MiB
   `nvtmpp` area and measure like the 3DNR pools. Until then, the plain graph should cap `main_fps` at what it
   proved (15) rather than starve the sub stream silently.
4. **Optional sub stream settings:** `sub_resolution` (640x480, 640x360), `sub_fps` (5 to 15), `sub_bitrate`;
   defaults as today (640x480@10, 512 kbit/s). Not needed for 30 fps, but it makes the sub stream adjustable in
   both directions, as asked. Live like the main settings (sub path restart); the ONVIF sub profile follows.
5. **Small fix found on the way:** `cam/src/onvif.rs` reports the main encoder as H.264 Main profile; it has been
   High since the stock encoder settings commit.

Tests: unit tests for the new defaults and settings; pool sizes in pipeline tests; a RAM session matrix like the
table above plus the snapshot path; then a 4-hour run in production.

## 2. ONVIF Profile T alongside Profile S

### Is T the replacement for S?

Yes. In October 2025 ONVIF announced the end of Profile S, recommending Profile T as its successor, mainly because
Profile S's authentication no longer meets current security advice. The June 2026 test tools are the last that let
products claim Profile S; deployed Profile S systems keep working. Clients do not stop working either: tinyCam,
Home Assistant and recorders call Media1 or Media2 whatever profiles a device claims in discovery.

So "in parallel" means: keep everything Profile S clients use (Media1, the current scopes), add what Profile T
mandates, and add the `onvif://www.onvif.org/Profile/T` scope only once all of it is there.

### Profile T v1.0 device requirements against what exists

The mandatory features are in section 7 of the specification [D]; section 8 lists conditional ones (PTZ presets and
home, audio, relays, auxiliary commands, focus, NTP), which the E1 Zoom already serves where it has the hardware.

| Feature (spec section) | What Profile T requires of a device | Today |
|---|---|---|
| User authentication (7.1) | HTTP Digest, RTSP Digest, Digest on RTSP tunnelled over HTTP | HTTP and RTSP Digest: yes. Tunnel: **no** |
| Capabilities (7.2) | `GetServices`, service capabilities incl. Media2, Imaging, Device IO; `GetWsdlUrl`; `MaxPullPoints` ≥ 2; `MaximumNumberOfProfiles` | mostly yes; check the two numbers |
| Discovery (7.3) | `Get/SetDiscoveryMode`, `Get/Add/Set/RemoveScopes`, the Profile T scope | reads only; **writes missing** |
| Network configuration (7.4) | Get **and Set** host name, DNS, interfaces, protocols, gateway | reads only |
| System (7.5) | device info, date and time, `SetSystemFactoryDefault`, `SystemReboot` | factory default **missing** |
| User handling (7.6) | `CreateUsers`, `GetUsers`, `SetUser`, `DeleteUsers` | one fixed user; **set and delete missing** |
| Event handling (7.7) | pull point, `GetEventProperties`, topic and message content filters, `Unsubscribe`, 2 concurrent subscriptions | pull point yes; filters to check |
| Media profile management (7.8) | `CreateProfile`, `DeleteProfile` that really work, events on change | refused today (fixed profiles) |
| Video streaming (7.9) | H.264 or H.265; RTP/UDP, RTSP over HTTP, **multicast**; key frame on `SetSynchronizationPoint` | UDP yes; **HTTP tunnel and multicast missing**; key frame: wire to the existing forced key frame |
| Video profile, source and encoder configuration (7.10 to 7.12) | `Add/RemoveConfiguration`, `SetVideoSourceConfiguration`, **`SetVideoEncoderConfiguration` that changes the stream**, events on each change | read-only today |
| Metadata streaming and configuration (7.13 to 7.15) | an ONVIF metadata RTP stream (`application/vnd.onvif.metadata`), metadata configurations that can be set | **missing** |
| Imaging settings (7.16) | `GetImagingSettings`, `GetOptions`, `SetImagingSettings` | yes |
| Tampering events (7.17) | at least one tampering event | **missing** |
| On-screen display (7.18) | create, list, change, delete text OSDs | **missing** (`GetOSDs` only) |
| JPEG snapshot (7.19) | `GetSnapshotUri` | yes |
| Motion alarm events (7.20) | motion alarm events | **missing** |
| Absolute and continuous PTZ (7.21, 7.22) | when PTZ exists | yes |

The current ONVIF doc says Profile T "also asks for HTTPS, H.265 and motion events". HTTPS and H.265 are wrong for
v1.0: HTTPS streaming is "if supported", and either codec is enough. That doc line gets fixed in the first step.

### Work, in independent pieces

1. **Operations without new machinery:** discovery mode and scope writes, network `Set*` (applied through the
   daemon's own network config, or refused field by field where the camera cannot do it), factory default (resets
   `cam-settings.toml`, keeps credentials and network), `SetUser`/`DeleteUsers` over the one account, event filter
   dialects, `SetSynchronizationPoint` forcing a key frame (the encoder restart already used for joining viewers).
2. **Streaming transports in `camera-transport`:** RTSP tunnelled over HTTP (the GET/POST pair with base64 RTSP,
   Digest on the RTSP level) and RTP multicast (a group address per profile, `Transport: RTP/AVP;multicast`, TTL).
3. **Editable media profiles with change events:** profiles become data the daemon persists, built from the fixed
   configurations (two video encoders, source, audio, PTZ, metadata). `SetVideoEncoderConfiguration` maps onto the
   `main_*` and `sub_*` settings, which is also the ONVIF way to switch codec (part 3) and frame rate (part 1).
   Each change sends the configuration-changed event.
4. **Metadata stream:** an RTSP track carrying `tt:MetadataStream` XML: PTZ status and the events below. Needs one
   more RTP payload type in the server and a metadata configuration in Media2.
5. **Motion and tampering events:** motion from a frame difference on the sub stream or the vendor's motion
   detection module (`kdrv_md` is in the firmware; its HDAL interface needs probing). Tampering from the same
   statistics: a covered or blinded lens (picture near black or uniform), or a sudden global change. Published as
   ONVIF events and as MQTT binary sensors.
6. **Text on-screen display:** the encoder has stamp inputs (`IN_STAMP_BUF`, `IN_STAMP_IMG`, `IN_STAMP_ATTR` in
   `libhdal` [D]); needs a small bitmap font renderer and the stamp ABI probed.
7. **Claim Profile T** in discovery, per camera: only cameras whose `Camera` implementation provides all of the
   above. The Mi 360 and the DS-KD8003 share the crate and would keep claiming S until they have motion, tampering
   and OSD.

Piece 1 is small. Pieces 2, 4 and 6 are medium each. Pieces 3 and 5 are the largest. Conformance can only be
proven with ONVIF's test tool (members only); without it, the target is "a Profile T client works", checked with
ONVIF Device Manager, Home Assistant and a recorder.

## 3. Switching between H.264 and H.265

HEVC is another name for H.265, so this is a switch between H.264 and H.265 per stream.

### What was measured [V]

- Encoder values: `codec_type` 2, profile 1 (Main), `level_idc` 153 (5.1), `entropy_mode` **2** (H.264's CABAC
  value 1 and 0 are refused with -34).
- Runs at 2592x1944 at 15 and 30 fps, with 3DNR (compressed reference input), gradual refresh and slice split;
  every frame delivered, ffmpeg decodes without errors, the full-range VUI flag is carried; CPU unchanged.
- Slice rows count 64-pixel coding units in H.265: 8 rows give 4 slices at 1944 high, against 16 in H.264.
- Key frames are type 19 (IDR) with VPS, SPS and PPS (types 32, 33, 34) in front.
- Picture quality, two encoders on the same frames at 1920x1440@15 against an H.264 8 Mbit/s reference: H.265 at
  0.5 Mbit/s matched H.264 at 1 Mbit/s (34.3 against 34.2 dB PSNR), H.265 at 1 Mbit/s nearly matched H.264 at
  2 Mbit/s (34.9 against 34.9 dB).

### Changes per component

| Component | Change |
|---|---|
| `hdal` (`enc.rs`, `sys/mod.rs`, `fake.rs`) | `EncSpec` gets a `codec`. `encoder_params` writes codec, profile, level and entropy per codec; slice rows and gradual refresh rows converted to the codec's row unit (coding units of 64 for H.265; the GDR row unit still has to be measured). Check whether `PATH_CONFIG`'s buffer sizing depends on the codec, which would make a switch a path close and reopen instead of an encoder restart. |
| `cam` pipeline | `MainStream` (and a future `SubStream`) carry the codec. A codec switch is a main path restart like a bitrate change; refuse nothing new (3DNR, GDR and slices all work with H.265). |
| `cam` settings and MQTT | `main_codec` select (`H.264`, `H.265`), default H.264, live. Later `sub_codec`. Discovery fixture gets the entity. `--retune` gets an `@h265` flag for sessions. |
| `cam dump` and `annexb.rs` | NAL type parsing per codec (H.265: `(byte >> 1) & 0x3f`), files named `.h265`. |
| `camera-transport` `annexb.rs` | NAL unit splitting is shared; type extraction and "is a key frame" per codec (H.265 IRAP types 16 to 21). |
| `camera-transport` hub | Parameter set cache: VPS, SPS and PPS for H.265 (types 32 to 34) instead of SPS and PPS; viewers start at an IRAP picture. A stream carries its codec. On a codec change the hub ends every session of that stream, since their SDP no longer describes it; clients reconnect. |
| `camera-transport` RTP | RFC 7798 packetization: single NAL unit packets and fragmentation units (type 49, a two-byte NAL header plus a one-byte FU header). Pacing is unchanged. |
| `camera-transport` SDP | `a=rtpmap:96 H265/90000`, `a=fmtp:96 sprop-vps=…;sprop-sps=…;sprop-pps=…` from the cached parameter sets. |
| `camera-onvif` | `VideoEncoding` gets the encoding (`H264` with its profile, `H265` with Main); Media2 reports `H265` and lists both in the encoder options. Media1 cannot describe H.265 (its schema has only JPEG, MPEG4 and H.264): choose between leaving an H.265 profile out of Media1, or listing it with no encoder configuration. Decision needed. |
| Other cameras | The shared crates stay H.264 by default; the Mi 360 and DS-KD8003 do not change behaviour. |
| Docs | settings.md (`main_codec`), network.md (packetization), shared/onvif.md (Media1 and Media2 behaviour), hdal-abi.md (codec values). |

### Tests

- Unit: RTP fragmentation and reassembly round trip for H.265, SDP text, hub key and parameter set handling,
  settings and MQTT.
- On the camera: RAM session with `@h265` retunes; ffmpeg over TCP and UDP; VLC; tinyCam on the phone; a codec
  switch while viewers are connected.
- Clients to check before making H.265 any default: tinyCam (fine per the phone), Home Assistant's stream
  component, Frigate or go2rtc, and Synology if used. Browsers often cannot show H.265 live, so recorders with a
  web view may need H.264 on the sub stream.

## Decisions

1. **Part 1 defaults:** H.265 is the default main codec, and with it 2592x1944, 30 fps and 6144 kbit/s. Settings
   saved by an older firmware keep their values (the E1 Zoom in testing had `main_fps = 15` and
   `main_bitrate = 4096` saved).
2. **Part 3:** the main stream is H.265 by default for the test; the sub stream stays H.264. `main_codec` switches
   it live, and the main stream's viewers reconnect.
3. **Media1 behaviour of an H.265 profile:** Media1's schema has no H.265 and H.265 reached ONVIF with Media2.
   Cameras that stream H.265 still put `H265` in Media1's `Encoding`, so this one does; Home Assistant's ONVIF
   integration only lists profiles that say `H264`, so there the main stream needs `main_codec` H.264. Media2 and so
   Profile T clients get `H265`.
4. **Part 2:** the new features come next, in the order of the work list above.

## Sources

- [ONVIF to End Support for Profile S; Recommends Profile T as Replacement](https://www.onvif.org/?p=8621)
- [ONVIF Profile T](https://www.onvif.org/profiles/profile-t/)
- [ONVIF Profile T Specification v1.0](https://www.onvif.org/wp-content/uploads/2018/09/ONVIF_Profile_T_Specification_v1-0.pdf)
- [ONVIF Profile Feature Overview v2.3](https://www.onvif.org/wp-content/uploads/2019/12/ONVIF_Profile_Feature_overview_v2-3.pdf)
- [RFC 7798, RTP Payload Format for HEVC](https://www.rfc-editor.org/rfc/rfc7798)
