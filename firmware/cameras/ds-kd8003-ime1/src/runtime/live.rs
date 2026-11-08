//! Live settings and telemetry on the board: the media backend of the apply owner, the speaker
//! restart that carries new clip/talk gains, the settings thread and the telemetry thread.
use super::audio::{self, Sound};
use super::ffi::*;
use super::hw::{StreamSettings, refresh_stream, set_stream};
use super::{Device, spawn_named};
use camera_transport::Talkback;
use camera_transport::mqtt::Publisher;
use camera_transport::settings::{FileDisk, JsonCodec};
use camera_transport::telemetry::Proc;
use ds_kd8003_ime1_cam::media::{Change, Media, Owner, Stream};
use ds_kd8003_ime1_cam::telemetry::{self, Counters, Isp, Telemetry};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

pub type Settings = Owner<FileDisk, JsonCodec>;

/// The speaker thread, restarted to change its gains: the old loop finishes its sound and ends when
/// its queue closes; the new one joins it first, so only one ever drives the speaker.
pub struct Speaker {
    g: Arc<Device>,
    talk: Arc<Talkback>,
    tx: Mutex<Option<mpsc::Sender<Sound>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Speaker {
    pub fn new(g: Arc<Device>, talk: Arc<Talkback>) -> Arc<Self> {
        Arc::new(Self {
            g,
            talk,
            tx: Mutex::new(None),
            thread: Mutex::new(None),
        })
    }

    /// Queues a sound; dropped when the speaker never started.
    pub fn play(&self, sound: Sound) {
        if let Some(tx) = self.tx.lock().unwrap().as_ref() {
            let _ = tx.send(sound);
        }
    }

    /// Starts the loop with these gains, replacing a running one.
    pub fn start(&self, clip_db: i32, talk_db: i32) {
        let (tx, rx) = mpsc::channel();
        // Dropping the old sender closes the old loop's queue once it is drained.
        *self.tx.lock().unwrap() = Some(tx);
        let previous = self.thread.lock().unwrap().take();
        let (g, talk) = (self.g.clone(), self.talk.clone());
        let handle = spawn_named("speaker", move || {
            if let Some(old) = previous {
                let _ = old.join();
                // A talk tail may have left the amplifier on.
                // SAFETY: plain ioctl wrapper.
                let rc = unsafe { gm_amp(0) };
                let mut s = g.st.lock().unwrap();
                s.amp = rc != 0 && s.amp;
                s.speaker_busy = false;
            }
            g.log(format!("speaker: clip {clip_db} dB, talk {talk_db} dB"));
            audio::speaker_loop(g, rx, clip_db, talk_db, talk);
        });
        *self.thread.lock().unwrap() = Some(handle);
    }

    fn running(&self) -> bool {
        self.tx.lock().unwrap().is_some()
    }
}

/// The apply owner's view of the board.
pub struct Board {
    pub g: Arc<Device>,
    pub speaker: Arc<Speaker>,
}

impl Media for Board {
    fn codec_set(&mut self, output: bool, db: i32) -> i32 {
        // SAFETY: plain acodec ioctl wrapper.
        unsafe { gm_codec_set(i32::from(output), db) }
    }

    fn codec_get(&mut self) -> Option<(i32, i32)> {
        let (mut input, mut output) = (0, 0);
        // SAFETY: two valid out-pointers.
        (unsafe { gm_codec_volume(&mut input, &mut output) } == 0).then_some((input, output))
    }

    fn speaker(&mut self, clip_db: i32, talk_db: i32) -> i32 {
        if !self.speaker.running() {
            return -1;
        }
        self.speaker.start(clip_db, talk_db);
        0
    }

    fn stream_set(&mut self, chn: usize, s: &Stream, how: Change) -> i32 {
        if how == Change::Attr {
            let rc = {
                let _guard = self.g.video_lock[chn].lock().unwrap();
                // SAFETY: plain value call; the channel's lock keeps its reader out.
                unsafe { gm_stream_attr(chn as i32, s.fps, s.kbps, s.gop, i32::from(s.vbr)) }
            };
            refresh_stream(&self.g, chn);
            self.g.log(format!(
                "stream {}: live {} fps {} kbit/s gop {} {} rc {rc}",
                if chn == 0 { "main" } else { "sub" },
                s.fps,
                s.kbps,
                s.gop,
                if s.vbr { "VBR" } else { "CBR" }
            ));
            return rc;
        }
        set_stream(
            &self.g,
            chn,
            &StreamSettings {
                w: s.w,
                h: s.h,
                fps: s.fps,
                kbps: s.kbps,
                gop: s.gop,
                vbr: s.vbr,
                profile: s.profile,
            },
        )
    }

    fn stream_get(&mut self, chn: usize) -> Option<Stream> {
        refresh_stream(&self.g, chn);
        let o = self.g.st.lock().unwrap().streams[chn]?;
        Some(Stream {
            w: o[0],
            h: o[1],
            fps: o[2],
            kbps: o[3],
            gop: o[4],
            vbr: o[5] == 2,
            profile: o[6],
        })
    }

    fn isp_set(&mut self, key: u32, raw: u32) -> i32 {
        let _guard = ISP.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: plain value call; the bridge checks the live ISP context and refuses key 1.
        let rc = unsafe { gm_isp_key(key as i32, raw) };
        if rc != 0 && key == ds_kd8003_ime1_cam::settings::ISP_SATURATION {
            // The direct setter kept night monochrome working before the key path.
            // SAFETY: plain value call into the stock ISP context.
            return unsafe { gm_saturation(raw as i32) };
        }
        rc
    }

    fn isp_get(&mut self, key: u32) -> Option<u32> {
        isp_read(key)
    }
}

/// The vendor ISP context is shared: one key call at a time (settings and telemetry threads).
static ISP: Mutex<()> = Mutex::new(());

fn isp_read(key: u32) -> Option<u32> {
    let _guard = ISP.lock().unwrap_or_else(|e| e.into_inner());
    let mut v = 0u32;
    // SAFETY: one valid out-pointer; the bridge reads into its own larger buffer.
    (unsafe { gm_isp_get(key as i32, &mut v) } == 0).then_some(v)
}

pub enum Msg {
    Set { slot: u16, value: i32 },
    Night(bool),
}

/// Runs the apply owner: commands and day/night in, states out, saves within the window, a flush
/// on stop.
pub fn run_settings(
    g: Arc<Device>,
    mut owner: Settings,
    mut board: Board,
    rx: mpsc::Receiver<Msg>,
    publisher: Option<Publisher>,
) -> JoinHandle<()> {
    spawn_named("settings", move || {
        let publish = |states: Vec<(&'static str, String)>| {
            if let Some(p) = &publisher {
                for (id, value) in states {
                    if let Err(e) = p.state(id, value) {
                        g.log(format!("mqtt: {e}"));
                    }
                }
            }
        };
        publish(owner.states());
        while !g.stop.load(Ordering::Relaxed) {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Msg::Set { slot, value }) => {
                    let states = owner.set(&mut board, slot, value, g.t0.elapsed());
                    publish(states);
                }
                Ok(Msg::Night(night)) => owner.night(&mut board, night),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            owner.tick(g.t0.elapsed());
            for line in owner.take_log() {
                g.log(line);
            }
        }
        owner.flush();
        for line in owner.take_log() {
            g.log(line);
        }
    })
}

/// Samples every 30 s and publishes what moved beyond its deadband.
pub fn run_telemetry(g: Arc<Device>, publisher: Publisher) {
    spawn_named("telemetry", move || {
        let mut t = Telemetry::new(Proc::default());
        let mut next = Duration::ZERO;
        while !g.stop.load(Ordering::Relaxed) {
            let now = g.t0.elapsed();
            if now < next {
                std::thread::sleep(Duration::from_millis(500));
                continue;
            }
            next = now + telemetry::EVERY;
            let counters = {
                let s = g.st.lock().unwrap();
                Counters {
                    frames: s.stream_frames,
                    bytes: s.stream_bytes,
                }
            };
            let isp = Isp {
                luma: isp_read(telemetry::KEY_LUMA),
                night: isp_read(telemetry::KEY_NIGHT),
            };
            for (id, value) in t.round(now, counters, isp, publisher.is_connected()) {
                if let Err(e) = publisher.state(id, value) {
                    g.log(format!("mqtt: {e}"));
                }
            }
        }
    });
}
