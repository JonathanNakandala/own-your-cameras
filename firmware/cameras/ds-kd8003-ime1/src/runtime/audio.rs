//! Microphone capture into the RTSP hub (PCMU 8 kHz) and speaker playback of named clips. The
//! codec runs AI and AO at one rate (16 kHz wanted, 8 kHz if the switch fails); the shared
//! microphone fan-out decimates the microphone to 8 kHz for PCMU, and the clips (shipped at 16 kHz)
//! go through the same decimator when the codec is at 8 kHz.
use super::Device;
use super::ffi::*;
use camera_transport::AudioHub;
use camera_transport::Talkback;
use camera_transport::microphone::MicFanout;
use camera_transport::resample::Decimator;
use ds_kd8003_ime1_cam::call::Tone;
use ds_kd8003_ime1_cam::speaker::{self, Frame, Scheduler};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

/// The codec rate asked for at start; clips are shipped at this rate.
pub const WANTED_RATE: u32 = 16000;
/// Longest clip kept: 10 s.
const MAX_CLIP_SAMPLES: usize = 10 * WANTED_RATE as usize;

static CODEC_RATE: AtomicU32 = AtomicU32::new(8000);

pub fn set_codec_rate(rate: u32) {
    CODEC_RATE.store(rate, Ordering::Relaxed);
}

pub fn codec_rate() -> u32 {
    CODEC_RATE.load(Ordering::Relaxed)
}

/// 40 ms of samples at the codec rate.
fn frame_len() -> usize {
    codec_rate() as usize / 25
}

/// A 16 kHz clip at 8 kHz, through the shared low-pass decimator. The tail is flushed with silence so
/// the clip keeps its full length.
fn halve(samples: &[i16]) -> Vec<i16> {
    let Some(mut decimator) = Decimator::new(WANTED_RATE) else {
        return Vec::new();
    };
    let mut padded = samples.to_vec();
    padded.extend(std::iter::repeat_n(0, 256));
    let mut out = decimator
        .push(0, &padded)
        .map(|(_, s)| s)
        .unwrap_or_default();
    out.truncate(samples.len() / 2);
    out
}

pub enum Sound {
    Clip(String, Arc<Vec<i16>>),
    /// A short two-tone refusal.
    Deny,
    /// A generated call tone.
    Tone(Tone),
    /// Start the call's ringback loop at this gain (dB), or stop it.
    Ringback(Option<(Arc<Vec<i16>>, i32)>),
}

pub fn capture_loop(
    g: Arc<Device>,
    hub: Arc<AudioHub>,
    reader: camera_transport::settings::Reader<'static>,
) {
    // PCMU only: the HiSilicon V3 runs no Opus encoder (see the camera README).
    let Ok(mut fanout) = MicFanout::<camera_opus::Encoder>::new(codec_rate(), None, Some(hub))
    else {
        g.log(format!("mic: no PCMU decimation from {} Hz", codec_rate()));
        return;
    };
    let mut pcm = vec![0u8; 8192];
    let mut presence = ds_kd8003_ime1_cam::presence::Presence::new(codec_rate());
    let mut frames_seen = 0u32;
    while !g.stop.load(Ordering::Relaxed) {
        let (mut len, mut pts) = (0u32, 0u64);
        let rc = {
            let _guard = g.audio_lock.read().unwrap();
            // SAFETY: pcm has 8192 bytes; the bridge copies at most cap bytes.
            unsafe { gm_audio(pcm.as_mut_ptr(), pcm.len() as u32, &mut len, &mut pts) }
        };
        if rc != 0 {
            g.st.lock().unwrap().audio_err = rc;
            if rc == -9 {
                g.log("mic: uncertain lease, capture stopped");
                g.media_dead.store(true, Ordering::Relaxed);
                return;
            }
            thread::sleep(Duration::from_millis(20));
            continue;
        }
        let mut samples: Vec<i16> = pcm[..len as usize]
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        if samples.len() != frame_len() {
            continue;
        }
        let peak = samples.iter().map(|&x| (x as i32).abs()).max().unwrap_or(0);
        {
            let mut s = g.st.lock().unwrap();
            s.audio_frames += 1;
            s.mic_peak = peak as u32;
        }
        // The presence boost follows the live setting (read about once a second).
        if frames_seen.is_multiple_of(25) {
            let db = reader
                .get(ds_kd8003_ime1_cam::settings::MIC_PRESENCE_DB)
                .unwrap_or(0);
            if db != presence.db() {
                presence.set(db);
                g.log(format!("mic: presence boost {db} dB"));
            }
        }
        frames_seen = frames_seen.wrapping_add(1);
        presence.process(&mut samples);
        fanout.push(pts, &samples);
    }
}

/// Loads every `<name>.pcmd` (lossless residuals, as the installer ships them, see `clip`) or `<name>.pcm`
/// (raw s16le mono)
/// at `WANTED_RATE` from the clips directory, decimated
/// when the codec ended up at 8 kHz.
pub fn load_clips(g: &Device, dir: &std::path::Path) -> HashMap<String, Arc<Vec<i16>>> {
    let mut out = HashMap::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        g.log(format!("clips: cannot read {}", dir.display()));
        return out;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let residual = match path.extension().and_then(|e| e.to_str()) {
            Some("pcmd") => true,
            Some("pcm") => false,
            _ => continue,
        };
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let samples: Vec<i16> = if residual {
            ds_kd8003_ime1_cam::clip::decode(&bytes, MAX_CLIP_SAMPLES)
        } else {
            bytes
                .chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]))
                .take(MAX_CLIP_SAMPLES)
                .collect()
        };
        if samples.is_empty() {
            continue;
        }
        let samples = if codec_rate() == WANTED_RATE / 2 {
            halve(&samples)
        } else {
            samples
        };
        g.log(format!(
            "clips: {name} {} ms",
            samples.len() * 1000 / codec_rate() as usize
        ));
        out.insert(name.to_string(), Arc::new(samples));
    }
    out
}

fn deny_tone() -> Vec<i16> {
    let rate = codec_rate() as usize;
    let n = rate * 6 / 10;
    (0..n)
        .map(|i| {
            let t = i as f32 / rate as f32;
            let on = (i / (rate / 5)).is_multiple_of(2);
            if on {
                (speaker::TONE_AMPLITUDE * (2.0 * std::f32::consts::PI * 400.0 * t).sin()) as i16
            } else {
                0
            }
        })
        .collect()
}

fn send(g: &Device, frame: &[i16]) -> i32 {
    let mut bytes = Vec::with_capacity(frame.len() * 2);
    for s in frame {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    let _guard = g.audio_lock.read().unwrap();
    // SAFETY: bytes is alive for the (copying) send.
    unsafe { gm_ao_send(bytes.as_ptr(), bytes.len() as u32) }
}

fn amp(g: &Device, on: bool) {
    // SAFETY: plain ioctl wrapper in the bridge.
    let rc = unsafe { gm_amp(on as i32) };
    g.st.lock().unwrap().amp = on && rc == 0;
}

/// Amplifier stays on this long after the last talk block, so word gaps do not click it.
const TALK_HOLD: Duration = Duration::from_millis(400);
/// Silent 40 ms frames queued ahead of talk: the hardware-side jitter cushion (and added latency).
const TALK_PREFILL_FRAMES: usize = 2;
/// Talk samples held back while the AO refuses frames, in 40 ms frames.
const TALK_BUF_FRAMES: usize = 4;

/// Silent frames after the last scheduled sound let the amplifier settle before it is switched off.
const SETTLE_FRAMES: usize = 6;

fn gained(samples: &[i16], gain: f32) -> Vec<i16> {
    if gain == 1.0 {
        return samples.to_vec();
    }
    samples
        .iter()
        .map(|&v| (v as f32 * gain).clamp(-32768.0, 32767.0) as i16)
        .collect()
}

/// The hardware side of [`speaker::Scheduler`]: one 40 ms frame at a time, so a stop or a one-shot
/// takes effect at the next frame. One-shots (clips, tones) play over the looping ringback; talk
/// (the RTSP backchannel mixer) plays only when neither does and is discarded while they do. The
/// amplifier is on only while something plays.
pub fn speaker_loop(
    g: Arc<Device>,
    rx: mpsc::Receiver<Sound>,
    clip_db: i32,
    talk_db: i32,
    talk: Arc<Talkback>,
) {
    let gain = 10f32.powf(clip_db as f32 / 20.0);
    // Voice must carry across the street: the mixer already holds speech at -12 dBFS RMS with a
    // -1 dBFS ceiling, so anything more is loudness bought with clipping, deliberately.
    let talk_gain = 10f32.powf(talk_db as f32 / 20.0);
    let rate = codec_rate();
    let deny = deny_tone();
    let tone = |t: Tone| match t {
        Tone::Answered => speaker::tone(speaker::ANSWERED, rate, speaker::TONE_AMPLITUDE),
        Tone::Declined => speaker::tone(speaker::DECLINED, rate, speaker::TONE_AMPLITUDE),
        Tone::Ended => speaker::tone(speaker::ENDED, rate, speaker::TONE_AMPLITUDE),
        Tone::Missed => deny.clone(),
    };
    let frame_len = frame_len();
    let mut sched = Scheduler::new();
    let mut ring_gain = 1.0f32;
    // A scheduled sound has the amplifier; `settle` counts its silent tail.
    let mut scheduled = false;
    let mut settle = 0usize;
    let mut talk_buf: Vec<i16> = Vec::with_capacity(frame_len * 2);
    let mut talk_amp_until: Option<Instant> = None;
    loop {
        if g.stop.load(Ordering::Relaxed) {
            if scheduled || talk_amp_until.is_some() {
                amp(&g, false);
                g.st.lock().unwrap().speaker_busy = false;
            }
            return;
        }
        let wait = if sched.busy() || sched.ringing() || settle > 0 {
            Duration::ZERO
        } else {
            Duration::from_millis(5)
        };
        let mut incoming = Vec::new();
        match rx.recv_timeout(wait) {
            Ok(sound) => incoming.push(sound),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if scheduled {
                    amp(&g, false);
                    g.st.lock().unwrap().speaker_busy = false;
                }
                return;
            }
        }
        incoming.extend(rx.try_iter());
        for sound in incoming {
            let (what, samples) = match sound {
                Sound::Clip(name, s) => (format!("clip {name}"), gained(&s, gain)),
                Sound::Deny => ("deny tone".to_string(), gained(&deny, gain)),
                Sound::Tone(t) => (format!("tone {t:?}"), gained(&tone(t), gain)),
                Sound::Ringback(Some((s, db))) => {
                    ring_gain = 10f32.powf(db as f32 / 20.0);
                    sched.ringback(Some(s));
                    g.log(format!("speaker: ringback start ({db} dB)"));
                    continue;
                }
                Sound::Ringback(None) => {
                    if sched.ringing() {
                        sched.ringback(None);
                        g.log("speaker: ringback stop");
                    }
                    continue;
                }
            };
            g.log(format!("speaker: start {what}"));
            sched.play(Arc::new(samples));
        }
        let (frame, ringback) = match sched.frame(frame_len) {
            Frame::OneShot(f) => (Some(f), false),
            Frame::Ringback(f) => (Some(f), true),
            Frame::Talk => (None, false),
        };
        if let Some(frame) = frame {
            if talk_amp_until.take().is_some() {
                // A scheduled sound interrupts the talk tail; the amplifier is already on.
                talk_buf.clear();
            } else if !scheduled {
                amp(&g, true);
            }
            if !scheduled {
                g.st.lock().unwrap().speaker_busy = true;
            }
            scheduled = true;
            settle = SETTLE_FRAMES;
            // Talk under a scheduled sound is dropped, not played late.
            while talk.next_pcm(Instant::now()).is_some() {}
            let frame = if ringback {
                gained(&frame, ring_gain)
            } else {
                frame
            };
            let rc = send(&g, &frame);
            g.st.lock().unwrap().speaker_rc = rc;
            if rc != 0 {
                thread::sleep(Duration::from_millis(40));
            }
            continue;
        }
        // Talk path.
        // Take every block that is due, not one: `send` blocks for a whole 40 ms frame once the
        // AO queue is full, and one 20 ms block per pass drained the mixer at half speed (speech
        // a second late, then chopped as its buffer overflowed).
        let mut got = false;
        while let Some(block) = talk.next_pcm(Instant::now()) {
            got = true;
            // No soft-float pass at 0 dB: the ARM926 has no FPU and this runs per sample.
            if talk_db == 0 {
                talk_buf.extend_from_slice(&block);
            } else {
                talk_buf.extend(
                    block
                        .iter()
                        .map(|&v| (v as f32 * talk_gain).clamp(-32768.0, 32767.0) as i16),
                );
            }
        }
        if scheduled && got {
            // Talk right after a scheduled sound keeps the amplifier: no settle, no click.
            scheduled = false;
            settle = 0;
            g.st.lock().unwrap().speaker_plays += 1;
            talk_amp_until = Some(Instant::now());
            g.log("speaker: done, talk follows");
        }
        if scheduled {
            if settle > 0 {
                settle -= 1;
                let _ = send(&g, &vec![0; frame_len]);
                if settle > 0 {
                    continue;
                }
            }
            amp(&g, false);
            scheduled = false;
            {
                let mut s = g.st.lock().unwrap();
                s.speaker_busy = false;
                s.speaker_plays += 1;
            }
            g.log("speaker: done");
        }
        let now = Instant::now();
        if got {
            if talk_amp_until.is_none() {
                amp(&g, true);
                g.st.lock().unwrap().speaker_busy = true;
                g.log("speaker: talk start");
                // The AO queue is the last cushion: with only the mixer's 40 ms lead in it, this thread
                // waking 30 ms late on the busy CPU left the DAC without samples (a click-gap in speech).
                // Silence ahead of the first frame keeps it TALK_PREFILL_FRAMES deeper for the session.
                for _ in 0..TALK_PREFILL_FRAMES {
                    let _ = send(&g, &vec![0; frame_len]);
                }
            }
            talk_amp_until = Some(now + TALK_HOLD);
            while talk_buf.len() >= frame_len {
                let frame: Vec<i16> = talk_buf.drain(..frame_len).collect();
                let rc = send(&g, &frame);
                if rc != 0 {
                    g.st.lock().unwrap().speaker_rc = rc;
                    break;
                }
            }
            // A failing AO must not grow the buffer without bound: keep the latest frames only.
            if talk_buf.len() > frame_len * TALK_BUF_FRAMES {
                let excess = talk_buf.len() - frame_len * TALK_BUF_FRAMES;
                talk_buf.drain(..excess);
            }
        } else if let Some(until) = talk_amp_until
            && now >= until
        {
            // Pad the tail to a whole frame, then let the amplifier go.
            if !talk_buf.is_empty() {
                talk_buf.resize(frame_len, 0);
                let _ = send(&g, &talk_buf);
                talk_buf.clear();
            }
            amp(&g, false);
            talk_amp_until = None;
            g.st.lock().unwrap().speaker_busy = false;
            g.log("speaker: talk end");
        }
    }
}
