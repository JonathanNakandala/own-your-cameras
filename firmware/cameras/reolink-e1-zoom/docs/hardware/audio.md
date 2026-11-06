# Audio: microphone, speaker and the amplifier

`cam serve` captures 48 kHz mono PCM from the microphone, encodes it to Opus, and plays 48 kHz signed 16-bit mono
samples on the speaker (capture and playback share one codec clock: with the speaker at 16 kHz, a 48 kHz capture
delivered frames at the 16 kHz rate [V]), inside the video pipeline's HDAL session and common memory. Both RTSP paths share one
microphone encoder and one Opus backchannel mixer for up to four talkers
([../architecture.md](../architecture.md#rtsp)). The RTSP server starts only after audio
startup succeeds; shutdown stops audio before common memory is torn down, and a failed start releases everything it
acquired. Tags as in [README.md](README.md#evidence-tags).

## Stock settings [D]

Stock `device` at `0x8f0f4..0x8f194` opens capture at 16 kHz, 16 bit, mono, 1,024 samples, 10 frames; `cam` uses
48 kHz with the same frame size and count (the capture unit reports 8–48 kHz in `SYSCAPS`).
ANR is `(enabled=1, suppress=30, HPF=160, bias=9)`. The AEC maximum settings are `(enabled=1, leak_enabled=0,
leak_value=99, noise=-60, echo=-60, filter_length=1024, frame_size=128, notch_radius=992, loopback=left)`; stock
also has a model-47 branch that picks noise −30, and the model predicate was not resolved, so `cam` uses the
generic −60. The stock runtime setter at `0x8f4d4..0x8f52c` uses the same filter, frame and notch values. The
kernel accepts these parameters [V]; that does not prove acoustic echo suppression.

**AEC has no reference signal.** `lb_channel=left` names the capture channel that carries the speaker loopback,
but capture is one channel (`mode=1`) with a zeroed `DRV_CONFIG`, so nothing is mixed in. The stock
`DRV_CONFIG` contents were not recovered. Until a recording with the speaker playing shows the echo with AEC on
and off, treat the microphone path as having no echo reference. No loopback route is added: an unverified one
could change the raw capture frame layout. At 48 kHz the driver accepts AEC and ANR on or off and keeps
delivering exactly 48,000 samples per second [V]; whether they process anything at that rate is not established.

## Capture

No audio encoder unit is opened: the 16 kHz `hd_audioenc` AAC path is gone. `hd_audiocap_pull_out_buf` on the
capture i/o path returns raw frames of 1,024 samples (2,048 bytes) with a 48 kHz PTS. Their physical addresses are
not page aligned, but all lie in one page-aligned ring that `hd_audiocap_get(ctrl, HD_AUDIOCAP_PARAM_BUFINFO)`
reports (49,920 bytes on the tested camera) [V]. The ring is mapped once, each frame is bounds-checked against it,
cache-invalidated before the copy, and every pulled descriptor is released, also on the copy and cache failure paths.

The media thread regroups the frames into 20 ms (960-sample) frames and encodes each with libopus 1.5.2
(48 kHz mono, `OPUS_APPLICATION_AUDIO`, complexity 5, 64 kbit/s, built with NEON). One encoder serves every
listener; the audio hub fans the packets out, and the RTSP sender paces them at 20 ms.

## Speaker

Samples go through a 2,048-byte physical staging buffer from `hd_common_mem_alloc`, flushed before the
non-blocking `hd_audioout_push_in_buf`; no Rust heap pointer reaches an HDAL descriptor. The staging buffer is
reusable as soon as the push returns [D]: `isf_audout_inputport_push_imgbuf_to_dest` (`kflow_audioout.ko`
`0x1748`) calls `wavstudio_push_buf_to_queue`, which (`kflow_audiocap.ko` `0x37f0`) copies the bytes into its own
ring at `0x3940` before returning. A full buffer counts as dropped samples; other failures propagate.

Stock allocates 60 output frames (`device` `0x8fe68`); `cam` uses eight. At 16 kHz, three frames dropped about
40 % of paced samples and eight dropped none on the same recording [V]; at 48 kHz eight frames hold 170 ms, and
tone and speech runs dropped none [V]. That is allocation capacity, not a measured playback latency.

Each loop turn the hardware loop waits at most 10 ms for video, takes every pending capture frame, and takes every
due 20 ms block (960 samples) from the talkback mixer, which emits blocks on its own real-time clock. The speaker
gets whole 1,024-sample frames, the driver's `frame_sample_max`: pushing 960-sample blocks played with audible
jitter [V]. A burst's last partial frame is padded with silence. Each burst logs
`speaker: burst ended played_ms=… dropped_ms=…`.

### Amplifier: GPIO 234

The codec output alone is too weak for the speaker. Stock talkback drives GPIO **234** high during playback
and low afterwards [V, syscall trace of the stock `device`]. With the same voice clip replayed and recorded by
the camera's own microphone, speech correlation was 0.094 with GPIO 234 low, 0.532 with it high and 0.553 on
stock [V].

`Sys::with_speaker_enable` takes the sysfs GPIO adapter. Speaker startup first drives it low, enables it once
HDAL and the staging memory are ready, and keeps it high for the output graph's lifetime; teardown mutes it
before releasing HDAL. A failed enable mutes and unwinds; a failed mute still releases every HDAL resource and
ends the session. Without the GPIO the adapter refuses to start the speaker.

## Tuning at runtime

Every audio parameter is a setting ([../settings.md](../settings.md)): mic and speaker volume, AEC on/off with
noise, echo, notch and leak, ANR on/off with suppression, high-pass cutoff and bias. `hdal::audio::Tuning::stock()`
holds the stock values (AEC and ANR on, mic 80 as stock sets it through ctrl item `0x19001`, speaker 100). The
`DEV_CONFIG` `aec_max`/`anr_max` envelope always stays stock.

- **Live sets do not take effect** [V]. On a running capture, `hd_audiocap_set(OUT_AEC/OUT_ANR)` on the i/o path
  and `VOLUME` on the ctrl path return 0, but 20 s quiet-room captures were unchanged (−47.9/−48.0/−48.1 dBFS RMS
  for mic 100/50/100, −48.0 with ANR on). Stock also re-opens the capture for every AEC/ANR change.
- **So the media thread restarts the capture path** between loop iterations; video, the sub stream and the
  speaker keep running. The same settings then change the capture by up to 23 dB (ANR on −38.9 vs off −24.6 dBFS
  RMS) [V]. The room was not acoustically controlled: this shows the settings take effect, not good values.
- Speaker volume is a live ctrl-path set on the running output, no restart.
- **With the codec's ALC on, HDAL's mic volume and the speaker volume change nothing measurable** [V]. A camera
  recording replayed at +12 dB through the backchannel and recorded from `Preview_01_sub` (peaks −4…−6 dBFS, no
  clipping) gave −23.1…−24.6 dBFS for HDAL mic volume 100 and 30, and −24.4/−24.5 dBFS for speaker 100 and 40.
- **The codec runs ALC and a noise gate.** Every capture start leaves the embedded codec (EAC, physical
  `0xF0640000`) with `0x30` bit 0 (ALC) and bit 1 (noise gate) set. Field names and layout come from disassembly
  of the vendor kernel's `eac_get_ad_config` and `nvt_audio.ko`'s `eac_dump_info`. A write takes effect after
  `1` is written to the load latch `0x04` (`eac_set_load`). That these two explain the flat level is inferred
  from the names and the measurements below.
- **`cam` clears both bits after each capture start and sets the analog PGA from `mic_volume`** (`hdal::codec`,
  `cam/src/eac.rs`). 0…100 maps linearly over the 5-bit PGA (`0x20` bits 0–4 and 8–12). Stock runs PGA 14;
  `Tuning::stock()`'s mic 80 is PGA 25. Measured with a −20 dBFS tone from the camera's own speaker, ALC and gate
  off, one run each [V]:

  | Setting | Tone in the mic |
  |---|---|
  | PGA 10 / 14 / 18 | −13.5 / −7.8 / −2.5 dBFS (about 1.4 dB a step) |
  | Boost (`0x10` bits 4–5) 1 instead of stock 2 | 11.6 dB lower; `cam` leaves boost at stock |
  | Quiet room, gate on → off (ALC on) | −41.2 → −28.9 dBFS RMS |
  | Quiet room, ALC on → off (gate off) | −28.9 → −55.6 dBFS RMS |

  Default `mic_volume` is 100 (PGA 31), chosen by ear. If `/dev/mem` cannot be written, `cam` logs `mic gain:`
  and runs with the codec as stock leaves it.
- HDAL's capture volume (ctrl item `0x19001`) scales the capture once the ALC is off: at 70 the tone came out
  2.9 dB lower than at 100 (one run each). `cam` keeps it at 100 so `mic_volume` is one gain.
- A start the driver refuses falls back to the previous tuning, and the state reports what runs. If that also
  fails, capture is down and it is a media error. At boot a refused persisted tuning is retried once with
  `Tuning::stock()`.
- Kernel limits (`kflow_audiocap.ko` strings): bias is clamped to 1–9, a volume above 100 becomes 100. Suppression
  3–35 and high-pass 0–1000 Hz are the ranges exercised on the camera; nothing outside them is established, so the
  settings clamp there.
- ABI: audiocap set `VOLUME` (id 9) is a one-word ctrl-path set (`hd_audiocap_set` @ `0xdddc` → kflow item
  `0x19001`), listed in `hdal-sys/abi/libhdal-2.00-param-sizes.tsv`.

## Checking it on a camera

`make device-serve CAMERA=<ssh host> AUTH=<file with user:password>` runs `cam serve --diagnostic` in RAM with the
vendor stack stopped and reboots afterwards ([../development.md](../development.md)); the diagnostic serves on
port 8554, the installed daemon on 554. The steps below need an ffmpeg built with libopus
(`ffmpeg -hide_banner -encoders | grep libopus`). While it runs, `ffprobe` both paths over TCP: H.264 plus Opus,
48,000 Hz. Record the microphone:

```sh
ffmpeg -rtsp_transport tcp -i 'rtsp://USER:PASSWORD@CAMERA_IP:8554/Preview_01_sub' \
  -map 0:a:0 -t 3 -c:a copy mic.opus
```

Play a recording on the speaker through the backchannel (the tool reads the credentials from the file and
never prints them):

```sh
ffmpeg -i speech.wav -ac 1 -ar 48000 -c:a libopus -b:a 64k speech.opus
python3 -I scripts/audio_backchannel.py --host CAMERA_IP --port 8554 --auth AUTH_FILE speech.opus
```

The sender negotiates DESCRIBE → SETUP of track 2 → PLAY, sends the file's Opus packets as RTP (PT 111, 48 kHz
clock) in real time with TCP NODELAY, then TEARDOWN; it accepts either path. Run two senders at once to hear the
mix. When the diagnostic ends, its `audio:` line reports `speaker_samples`: about the clip's seconds × 48,000,
rounded up to a multiple of 1,024, with zero `speaker_dropped_samples` for a paced run. The `talk:` and `speaker:`
lines show each session and burst.

These checks establish microphone Opus on both paths and recorded audio reaching the speaker driver. They do not
establish intelligibility, loudness or echo quality: listen through your own client for that.
