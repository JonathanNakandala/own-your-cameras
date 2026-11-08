//! The application on the board: stops the vendor processes, owns media and I/O, serves RTSP, runs
//! the keypad and PIN policy, talks MQTT, feeds the watchdog only while its own loop is alive.
mod audio;
mod ffi;
mod http;
mod hw;
mod keypad;
mod link;
mod live;
mod onvif;
mod uart;

use camera_transport::microphone::Format;
use camera_transport::mqtt;
use camera_transport::settings::{FileDisk, JsonCodec, Values};
use camera_transport::{
    AudioHub, Backchannel, Config as RtspConfig, Credentials, Hub, MAIN_PATH, MAIN_PCMU_PATH,
    SUB_PATH, SUB_PCMU_PATH, StreamPath,
};
use ds_kd8003_ime1_cam::access::{Access, Decision};
use ds_kd8003_ime1_cam::call::{self, Call, CallId, DeviceId, Effect, Input, Limits, Tone};
use ds_kd8003_ime1_cam::call_link::{self, LinkEvent};
use ds_kd8003_ime1_cam::call_wire::{self, Context};
use ds_kd8003_ime1_cam::config::{self, Config};
use ds_kd8003_ime1_cam::home::{self, Command, DoorCommand};
use ds_kd8003_ime1_cam::inputs::{Edge, Edges, LockPulse};
use ds_kd8003_ime1_cam::media::Owner;
use ds_kd8003_ime1_cam::settings::{self as live_settings, SPECS};
use ds_kd8003_ime1_cam::strike::Strike;
use ffi::*;
use hw::*;
use std::collections::VecDeque;
use std::ffi::CString;
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock, mpsc};
use std::thread;
use std::time::{Duration, Instant};

/// Every thread carries a name so `/proc/<pid>/task/*/comm` says what is eating the CPU.
fn spawn_named<F: FnOnce() + Send + 'static>(name: &str, f: F) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name(name.into())
        .spawn(f)
        .expect("thread")
}

const AU_CAP: usize = 1024 * 1024;
const WDT_FEED: u32 = 0x0000_5704;
/// Healthy for this long after start marks the boot good for the supervisor.
const HEALTHY_AFTER: Duration = Duration::from_secs(120);
/// After this many consecutive failed strike releases (one per 50 ms pass) the process exits so the unfed watchdog resets the board.
const MAX_RELEASE_FAILURES: u32 = 20;
/// Clip played instead of the built-in tone when a code is refused, if the payload ships it.
const DENIED_CLIP: &str = "denied";
/// One period of the call's ringback, looped while it rings (installer clip name).
const RINGBACK_CLIP: &str = "call_dial_tone";
/// The last missed call, next to `settings.json`.
const MISSED_FILE: &str = "missed.json";
/// stdout (the RAM log) stops growing past this; the in-memory ring keeps the latest lines.
const MAX_STDOUT_BYTES: usize = 4 << 20;

#[derive(Default)]
pub struct State {
    pub phase: String,
    pub media_rc: i32,
    pub ao_rc: i32,
    pub wdt_feeds: u64,
    pub viewers: usize,
    pub streams: [Option<[u32; 7]>; 2],
    pub stream_frames: [u64; 2],
    pub stream_bytes: [u64; 2],
    pub audio_frames: u64,
    pub audio_err: i32,
    pub mic_peak: u32,
    pub speaker_busy: bool,
    pub speaker_rc: i32,
    pub speaker_plays: u64,
    pub amp: bool,
    pub inputs: u32,
    pub tamper: u32,
    pub button_presses: u64,
    pub relay_mask: u32,
    pub strike: Strike,
    pub relay_rc: i32,
    pub relay_pulses: u64,
    pub release_failures: u32,
    pub ir_duty: u8,
    pub night: bool,
    pub keypad_phase: &'static str,
    pub keypad_polls: u64,
    pub keypad_restarts: u64,
    pub granted: u64,
    pub denied: u64,
    pub mqtt_online: bool,
    pub mqtt_drops: u64,
    pub healthy: bool,
    pub log: VecDeque<(u64, u64, String)>,
    pub next_log: u64,
}

pub struct Device {
    pub t0: Instant,
    pub st: Mutex<State>,
    /// Milliseconds since `t0`, wrapping; compared with wrapping arithmetic.
    pub heartbeat: AtomicU32,
    pub stop: AtomicBool,
    /// A video or audio lease became uncertain: media is gone until the board resets.
    pub media_dead: AtomicBool,
    pub stdout_bytes: AtomicUsize,
    pub alarm_fd: i32,
    pub ir_fd: i32,
    pub led_fd: i32,
    /// Capture and playback take it shared; a sample-rate change (not done here) would take it exclusively.
    pub audio_lock: RwLock<()>,
    /// One lock per VENC channel: reads vs reconfiguration.
    pub video_lock: [Mutex<()>; 2],
}

impl Device {
    pub fn log(&self, text: impl Into<String>) {
        let t = self.t0.elapsed().as_millis() as u64;
        let text = text.into();
        let line = format!("[{t:>9}] {text}");
        if self
            .stdout_bytes
            .fetch_add(line.len() + 1, Ordering::Relaxed)
            < MAX_STDOUT_BYTES
        {
            println!("{line}");
        }
        let mut s = self.st.lock().unwrap();
        let n = s.next_log;
        s.next_log += 1;
        s.log.push_back((n, t, text));
        while s.log.len() > 2000 {
            s.log.pop_front();
        }
    }
    pub fn beat(&self) {
        self.heartbeat
            .store(self.now_ms() as u32, Ordering::Relaxed);
    }
    /// Milliseconds since the last beat, wrap-safe.
    fn heartbeat_age_ms(&self) -> u32 {
        (self.now_ms() as u32).wrapping_sub(self.heartbeat.load(Ordering::Relaxed))
    }
    fn now_ms(&self) -> u64 {
        self.t0.elapsed().as_millis() as u64
    }
}

pub fn main(args: &[String]) {
    if args.len() == 2 && args[1] == "--version" {
        println!("ds-kd8003-ime1-cam {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if args.len() != 4 || args[1] != "--run" {
        eprintln!("usage: ds-kd8003-ime1-cam --run RUNTIME_DIR CONFIG_FILE");
        std::process::exit(2);
    }
    let runtime_dir = args[2].clone();
    let config_file = std::path::Path::new(&args[3]);
    let cfg = match Config::load(config_file) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config: {e:?}");
            std::process::exit(3);
        }
    };
    let credentials = match std::fs::read_to_string(&cfg.pins)
        .map_err(|_| config::Error::Io)
        .and_then(|t| config::parse_pins(&t))
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("pins: {e:?}");
            std::process::exit(3);
        }
    };
    // SAFETY: setting a signal disposition; SIGPIPE must not kill the process when a client goes away.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
    let g = Arc::new(Device {
        t0: Instant::now(),
        st: Mutex::new(State {
            phase: "starting".into(),
            keypad_phase: "off",
            ..Default::default()
        }),
        heartbeat: AtomicU32::new(0),
        stop: AtomicBool::new(false),
        media_dead: AtomicBool::new(false),
        stdout_bytes: AtomicUsize::new(0),
        alarm_fd: open_dev("/dev/DDM/alarmV2", O_RDWR),
        ir_fd: open_dev("/dev/DDM/pwmIR", O_RDWR),
        led_fd: open_dev("/dev/DDM/led", O_RDWR),
        audio_lock: RwLock::new(()),
        video_lock: [Mutex::new(()), Mutex::new(())],
    });
    // Live settings: never fail; a bad file or field falls back to defaults and is reported.
    let settings_file = live_settings::path(config_file);
    let (mut values, problems) = Values::load(SPECS, FileDisk::new(&settings_file), JsonCodec);
    for p in &problems {
        g.log(format!("settings: {p}"));
    }
    for line in live_settings::seed(&mut values, &cfg.deprecated, Duration::ZERO) {
        g.log(line);
    }
    let reader = values.reader();
    g.log(format!(
        "ds-kd8003-ime1-cam {}: start, {} PINs, open {} s, settings {}{}",
        env!("CARGO_PKG_VERSION"),
        credentials.len(),
        reader.value("open_seconds").unwrap_or(0),
        settings_file.display(),
        if values.first_run() {
            " (first run)"
        } else {
            ""
        }
    ));
    install_signal_stop(&g);

    // Watchdog: fed only while the main loop heartbeats (stale > 6 s => hardware reset to stock).
    let wdt_fd = Arc::new(AtomicI32::new(open_dev("/dev/DDM/wdt", O_RDWR)));
    g.log(format!(
        "wdt: open fd {} errno {}",
        wdt_fd.load(Ordering::Relaxed),
        if wdt_fd.load(Ordering::Relaxed) < 0 {
            errno()
        } else {
            0
        }
    ));
    {
        let (g, wdt_fd) = (g.clone(), wdt_fd.clone());
        spawn_named("wdt", move || {
            loop {
                let fd = wdt_fd.load(Ordering::Relaxed);
                if fd >= 0 && g.heartbeat_age_ms() < 6000 && !g.stop.load(Ordering::Relaxed) {
                    // SAFETY: feed takes no user buffer.
                    let rc = unsafe { libc::ioctl(fd, WDT_FEED as libc::c_ulong, 0) };
                    if rc == 0 {
                        g.st.lock().unwrap().wdt_feeds += 1;
                    }
                }
                thread::sleep(Duration::from_millis(2000));
            }
        });
    }
    g.beat();

    g.st.lock().unwrap().phase = "stopping vendor".into();
    stop_vendors(&g);
    g.beat();
    if wdt_fd.load(Ordering::Relaxed) < 0 {
        // The vendor may have held the node exclusively; it is free now.
        let fd = open_dev("/dev/DDM/wdt", O_RDWR);
        wdt_fd.store(fd, Ordering::Relaxed);
        g.log(format!(
            "wdt: reopen fd {fd} errno {}",
            if fd < 0 { errno() } else { 0 }
        ));
    }
    set_relay(&g, 0, "safe start");
    set_ir(&g, 0);

    g.st.lock().unwrap().phase = "starting media".into();
    let dir = CString::new(runtime_dir.as_str()).unwrap();
    // Media start takes seconds (ISP, VB pools); keep the watchdog fed meanwhile.
    let starting = Arc::new(AtomicBool::new(true));
    {
        let (g, starting) = (g.clone(), starting.clone());
        spawn_named("beat", move || {
            while starting.load(Ordering::Relaxed) {
                g.beat();
                thread::sleep(Duration::from_millis(1000));
            }
        });
    }
    // SAFETY: the bridge owns all SDK state; vendor owners are stopped.
    let media_rc = unsafe { gm_start(dir.as_ptr()) };
    // SAFETY: speaker path only after a successful AI/video start.
    let ao_rc = if media_rc == 0 {
        unsafe { gm_ao_start() }
    } else {
        -1
    };
    // The codec clocks AI and AO together; the bench qualified 8/16/32/48 kHz. 16 kHz gives the
    // speaker its full band for the clips; the microphone is decimated to 8 kHz for PCMU.
    let rate_rc = if ao_rc == 0 {
        // SAFETY: no audio thread runs yet; the bridge serializes its own SDK calls.
        unsafe { gm_audio_rate(audio::WANTED_RATE) }
    } else {
        -1
    };
    audio::set_codec_rate(if rate_rc == 0 {
        audio::WANTED_RATE
    } else {
        8000
    });
    g.log(format!(
        "audio: codec {} Hz (rate rc {rate_rc})",
        audio::codec_rate()
    ));
    {
        let mut s = g.st.lock().unwrap();
        s.media_rc = media_rc;
        s.ao_rc = ao_rc;
    }
    g.log(format!("media: start rc {media_rc}, speaker rc {ao_rc}"));
    starting.store(false, Ordering::Relaxed);
    g.beat();
    if media_rc != 0 {
        g.log("media: cannot start, exiting for the supervisor");
        std::process::exit(5);
    }
    refresh_stream(&g, 0);
    // Talkback: the shared Opus backchannel mixer (same crate and decoder as the other cameras),
    // decoding straight at the speaker's rate.
    let talk = camera_opus::talkback_high_passed(
        audio::codec_rate(),
        ds_kd8003_ime1_cam::speaker::TALK_HIGH_PASS_HZ,
    );
    let speaker = live::Speaker::new(g.clone(), Arc::clone(&talk));
    // Codec levels, both streams and the ISP keys (AWB auto among them: without it the image is
    // magenta) from the settings, through their one owner.
    let mut owner = Owner::new(values);
    let mut board = live::Board {
        g: g.clone(),
        speaker: speaker.clone(),
    };
    owner.boot(&mut board, g.t0.elapsed());
    for line in owner.take_log() {
        g.log(line);
    }
    g.beat();

    // SAFETY: the graph is up; the bridge creates VENC channel 2 once, bound beside the main encoder.
    let jpeg_rc = unsafe { gm_jpeg_open() };
    g.log(format!("snapshot: JPEG channel rc {jpeg_rc}"));
    // True while the SNTP keeper last found the clock in sync: only then is `relock_at` published.
    let clock_synced = Arc::new(AtomicBool::new(false));
    if !cfg.ntp.is_empty() {
        let keeper = camera_transport::sntp::Keeper {
            servers: cfg.ntp.clone(),
            log: |line| eprintln!("{line}"),
        };
        if let Err(e) = keeper.spawn_synced(Arc::new(AtomicBool::new(false)), clock_synced.clone())
        {
            g.log(format!("time: not started: {e}"));
        }
    }

    let video = Hub::new();
    let sub = Hub::new();
    let mic = AudioHub::with_format(Format::Pcmu8k);
    for (chn, hub) in [(0usize, video.clone()), (1usize, sub.clone())] {
        let g2 = g.clone();
        spawn_named(&format!("video{chn}"), move || video_loop(g2, chn, hub));
    }
    {
        let (g2, hub, mic_reader) = (g.clone(), mic.clone(), reader.clone());
        spawn_named("mic", move || audio::capture_loop(g2, hub, mic_reader));
    }
    match camera_transport::bind(cfg.rtsp_port) {
        Ok(listener) => {
            let rtsp = Arc::new(RtspConfig {
                // The paths every camera serves. Every path carries the PCMU microphone; the PCMU
                // paths also offer a PCMU backchannel to recorders, `main`/`sub` offer Opus.
                streams: [
                    (MAIN_PATH, &video, Backchannel::Opus),
                    (SUB_PATH, &sub, Backchannel::Opus),
                    (MAIN_PCMU_PATH, &video, Backchannel::Pcmu),
                    (SUB_PCMU_PATH, &sub, Backchannel::Pcmu),
                ]
                .map(|(path, hub, backchannel)| {
                    StreamPath::new(path, hub.clone())
                        .audio(Some(mic.clone()))
                        .backchannel(backchannel)
                })
                .into(),
                creds: Some(Credentials {
                    user: cfg.rtsp_user.clone(),
                    password: cfg.rtsp_password.clone(),
                }),
                talk: Some(Arc::clone(&talk)),
            });
            let g2 = g.clone();
            spawn_named("rtsp", move || {
                let stop = AtomicBool::new(false);
                let _ = camera_transport::serve(&listener, rtsp, &stop);
                g2.log("rtsp: server ended");
            });
            g.log(format!("rtsp: serving on {}", cfg.rtsp_port));
        }
        Err(e) => g.log(format!("rtsp: bind failed {e}")),
    }

    let clips = audio::load_clips(&g, &cfg.clips);
    if ao_rc == 0 {
        speaker.start(
            reader.get(live_settings::CLIP_DB).unwrap_or(0),
            reader.get(live_settings::TALK_DB).unwrap_or(0),
        );
    }

    // Call button: /dev/event0, ARM32 input_event (16 B), ordinary non-grabbing reads.
    let (button_tx, button_rx) = mpsc::channel::<bool>();
    {
        let g2 = g.clone();
        spawn_named("button", move || {
            let fd = open_dev("/dev/event0", O_RDONLY);
            if fd < 0 {
                g2.log(format!("button: open failed errno {}", errno()));
                return;
            }
            let mut ev = [0u8; 16];
            loop {
                // SAFETY: ev is 16 bytes.
                let n = unsafe { libc::read(fd, ev.as_mut_ptr() as *mut libc::c_void, 16) };
                if n != 16 {
                    thread::sleep(Duration::from_millis(500));
                    continue;
                }
                let (ty, code, val) = (
                    u16::from_le_bytes([ev[8], ev[9]]),
                    u16::from_le_bytes([ev[10], ev[11]]),
                    i32::from_le_bytes(ev[12..16].try_into().unwrap()),
                );
                if ty == 1 && code == 5 && (val == 0 || val == 1) {
                    let _ = button_tx.send(val == 1);
                }
            }
        });
    }

    // The input loop emits every edge once; MQTT (and later ONVIF) consume them.
    let edges = Edges::new();
    let (publisher, commands) = match link::start(g.clone(), &cfg, &edges) {
        Ok((publisher, commands)) => (Some(publisher), Some(commands)),
        Err(e) => {
            g.log(format!("mqtt: not started: {e}"));
            (None, None)
        }
    };
    let (settings_tx, settings_rx) = mpsc::channel();
    let settings_thread =
        live::run_settings(g.clone(), owner, board, settings_rx, publisher.clone());
    if let Some(p) = publisher.clone() {
        live::run_telemetry(g.clone(), p);
    }
    // The intercom call: one owner, fed below from the button, MQTT and the direct link.
    let missed_path = settings_file.with_file_name(MISSED_FILE);
    let missed = std::fs::read_to_string(&missed_path)
        .ok()
        .and_then(|t| call_wire::parse_missed_file(&t));
    let mut call = Call::new(random4(), missed);
    let boot = call::hex(&random4());
    let panel_id = DeviceId::panel();
    let (call_link, link_events) = if cfg.call_port == 0 {
        (None, None)
    } else {
        match TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, cfg.call_port)) {
            Ok(l) => {
                let g2 = g.clone();
                let (link, events) = call_link::serve(
                    l,
                    call_link::LinkConfig::new(cfg.rtsp_user.clone(), cfg.rtsp_password.clone()),
                    Arc::new(AtomicBool::new(false)),
                    move |line| g2.log(line),
                );
                g.log(format!("call-link: serving on {}", cfg.call_port));
                (Some(link), Some(events))
            }
            Err(e) => {
                g.log(format!("call-link: bind failed {e}"));
                (None, None)
            }
        }
    };
    let ringback = clips.get(RINGBACK_CLIP).cloned();
    if ringback.is_none() {
        g.log(format!(
            "call: no {RINGBACK_CLIP} clip, the call rings silently at the gate"
        ));
    }
    let publish_call = |call: &Call| {
        let now = g.now_ms();
        let ctx = Context {
            boot: boot.clone(),
            wall_now_ms: clock_synced.load(Ordering::Relaxed).then(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis() as u64)
            }),
            panel_connected: call_link.as_ref().is_some_and(|l| l.connected()),
            ring_timeout_s: reader.get(live_settings::CALL_RING_TIMEOUT).unwrap_or(45) as u32,
            max_talk_s: reader.get(live_settings::CALL_MAX_TALK).unwrap_or(3) as u32 * 60,
            panel_ring_volume: reader
                .get(live_settings::CALL_PANEL_RING_VOLUME)
                .unwrap_or(80) as u32,
        };
        let json = call_wire::state_json(&call.snapshot(now), &ctx);
        if let Some(l) = call_link.as_ref() {
            l.publish(&json);
        }
        if let Some(p) = publisher.as_ref() {
            let missed = call_wire::missed_json(call.missed().as_ref(), now);
            if let Err(e) = p
                .state(home::CALL_STATE, json)
                .and_then(|_| p.state(home::CALL_MISSED, missed))
            {
                g.log(format!("mqtt: {e}"));
            }
        }
    };
    // After a reboot the call is idle: overwrite a retained ringing, and publish the kept mark.
    publish_call(&call);
    let mut context_published = (false, 0, 0, 0);
    let emit = |edge: Edge| {
        let missed = edges.emit(edge.clone());
        if missed > 0 {
            g.log(format!("inputs: {missed} consumer(s) missed {edge:?}"));
        }
    };
    let (keypad_tx, keypad_rx) = mpsc::channel::<keypad::Report>();
    let _keypad = keypad::start(g.clone(), cfg.keypad_key.clone(), keypad_tx);
    if cfg.http_port != 0 {
        match TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, cfg.http_port)) {
            Ok(l) => {
                let g2 = g.clone();
                // One connection at a time: the replies are tiny and a thread per client could exhaust the board.
                spawn_named("http", move || {
                    for c in l.incoming().flatten() {
                        http::handle(g2.clone(), c);
                    }
                });
            }
            Err(e) => g.log(format!("http: bind failed {e}")),
        }
    }

    let onvif = start_onvif(&g, &cfg, jpeg_rc == 0);
    // ONVIF is the second consumer of the input edges (MQTT is the first); it ignores inputs it
    // does not advertise.
    if let Some(events) = onvif.as_ref().map(|service| service.events()) {
        let rx = edges.subscribe(64);
        let g2 = g.clone();
        spawn_named("onvif-edges", move || {
            while !g2.stop.load(Ordering::Relaxed) {
                if let Ok(edge) = rx.recv_timeout(Duration::from_millis(500))
                    && let Some((input, state)) = edge.binary()
                {
                    events.input(input, state);
                }
            }
        });
    }

    let mut access = Access::new(credentials);
    let mut last_inputs: Option<(u32, u32)> = None;
    let mut last_ae = Instant::now() - Duration::from_secs(5);
    let mut daynight = DayNight::default();
    let mut lock_published: Option<(u64, bool)> = None;
    let mut cover_published: Option<bool> = None;
    let mut health_written = false;
    g.st.lock().unwrap().phase = "running".into();
    g.log("ds-kd8003-ime1-cam: running");

    let announce = |name: &str, clip: &str, quiet: bool| {
        if quiet {
            return;
        }
        match clips.get(clip).or_else(|| clips.get("door_opened")) {
            Some(samples) => speaker.play(audio::Sound::Clip(clip.to_string(), samples.clone())),
            None => g.log(format!("speaker: no clip for {name}")),
        }
    };
    let refuse = || {
        speaker.play(match clips.get(DENIED_CLIP) {
            Some(samples) => audio::Sound::Clip(DENIED_CLIP.to_string(), samples.clone()),
            None => audio::Sound::Deny,
        });
    };
    let open = |g: &Device, reason: &str| {
        // The pulse length is a live setting: read it at each opening.
        let open_s = reader.get(live_settings::OPEN_SECONDS).unwrap_or(5).max(1) as u32;
        let rc = set_relay(g, STRIKE, reason);
        let mut s = g.st.lock().unwrap();
        s.strike.open(g.now_ms(), open_s);
        if rc == 0 {
            s.relay_pulses += 1;
        } else {
            // The output may have switched anyway: release it on the next pass.
            s.strike.close_now();
        }
        rc == 0
    };

    // Executes the call's effects in order: the speaker first, so the ringback starts before any
    // network I/O, then the strike, then publications.
    let run_call = |call: &mut Call, effects: Vec<Effect>| {
        for effect in effects {
            match effect {
                Effect::StartRingback => {
                    if let Some(r) = ringback.as_ref() {
                        let db = reader.get(live_settings::CALL_RINGBACK_DB).unwrap_or(0);
                        speaker.play(audio::Sound::Ringback(Some((r.clone(), db))));
                    }
                }
                Effect::StopRingback => speaker.play(audio::Sound::Ringback(None)),
                Effect::Tone(Tone::Missed) => refuse(),
                Effect::Tone(t) => speaker.play(audio::Sound::Tone(t)),
                Effect::OpenStrike => {
                    g.log("call: open");
                    if open(&g, "call open") {
                        announce("call", "door_opened", false);
                        emit(Edge::Remote);
                    } else {
                        refuse();
                    }
                }
                Effect::Changed => publish_call(call),
                Effect::MissedChanged => {
                    if clock_synced.load(Ordering::Relaxed) {
                        let wall = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map_or(0, |d| d.as_millis() as u64);
                        call.stamp_missed(wall);
                    }
                    save_missed(&g, &missed_path, call.missed().as_ref());
                }
                Effect::Event(e) => {
                    g.log(format!(
                        "call: {} {}{}",
                        e.kind.name(),
                        call::hex(&e.call_id),
                        e.reason
                            .map_or(String::new(), |r| format!(" ({})", r.name()))
                    ));
                    if let Some(p) = publisher.as_ref()
                        && let Err(err) = p.event(home::CALL_EVENT, call_wire::event_json(&e))
                    {
                        g.log(format!("mqtt: {err}"));
                    }
                }
            }
        }
    };
    let fresh_id = || -> CallId { random4() };

    while !g.stop.load(Ordering::Relaxed) {
        let now = g.now_ms();
        let now_s = now / 1000;
        g.beat();

        // Strike pulse end. A failed release is retried every pass; the pulse stays due until the
        // hardware confirms the strike is off. Bits on without a pulse (a stray energize) go too.
        let expired = {
            let s = g.st.lock().unwrap();
            s.relay_mask != 0 && (s.strike.due(now) || s.strike.pulse(now).is_none())
        };
        if expired && set_relay(&g, 0, "pulse end") == 0 {
            let mut s = g.st.lock().unwrap();
            s.release_failures = 0;
            s.strike.released();
        } else if expired {
            let failures = {
                let mut s = g.st.lock().unwrap();
                s.release_failures += 1;
                s.release_failures
            };
            if failures >= MAX_RELEASE_FAILURES {
                g.log("relay: cannot release the strike, exiting so the watchdog resets the board");
                save_settings(&g, &settings_thread);
                std::process::exit(6);
            }
        }
        if g.media_dead.load(Ordering::Relaxed) && g.st.lock().unwrap().relay_mask == 0 {
            g.log("media: lease lost, exiting for a reboot");
            save_settings(&g, &settings_thread);
            std::process::exit(7);
        }
        // Published on every pulse change (open, restart, close, release) and strike bit flip; the
        // pulse is read here, at emit, so channel delay stays out of the value.
        let (unlocked, generation, pulse) = {
            let s = g.st.lock().unwrap();
            (
                s.relay_mask & STRIKE != 0,
                s.strike.generation(),
                s.strike.pulse(now),
            )
        };
        if lock_published != Some((generation, unlocked)) {
            let pulse = pulse.map(|p| LockPulse {
                pulse_ms: p.pulse_ms,
                remaining_ms: p.remaining_ms,
                relock_at: clock_synced
                    .load(Ordering::Relaxed)
                    .then(|| std::time::SystemTime::now() + Duration::from_millis(p.remaining_ms)),
            });
            emit(Edge::Lock(unlocked, pulse));
            lock_published = Some((generation, unlocked));
        }

        // Inputs: reed (bit 0), tamper.
        let (mut im, mut tm) = ([0u8; 64], [0u8; 64]);
        let irc = ioctl_buf(g.alarm_fd, ALARM_INPUTS, &mut im);
        let trc = ioctl_buf(g.alarm_fd, ALARM_TAMPER, &mut tm);
        if irc == 0 && trc == 0 {
            let (inputs, tamper) = (
                u32::from_le_bytes(im[..4].try_into().unwrap()),
                u32::from_le_bytes(tm[..4].try_into().unwrap()),
            );
            {
                let mut s = g.st.lock().unwrap();
                s.inputs = inputs;
                s.tamper = tamper;
            }
            if last_inputs.map(|(i, _)| i & 1) != Some(inputs & 1) {
                let reed_open = inputs & 1 != 0;
                g.log(format!(
                    "input: reed {}",
                    if reed_open { "OPEN" } else { "closed" }
                ));
                emit(Edge::Reed(reed_open));
            }
            // The cover is "open" while the strike is released or the leaf is away from the reed:
            // Home Assistant only offers "close" on an open cover, and a released strike is what
            // "close now" ends.
            let cover_open = inputs & 1 != 0 || unlocked;
            if cover_published != Some(cover_open) {
                emit(Edge::Cover(cover_open));
                cover_published = Some(cover_open);
            }
            if let Some((_, pt)) = last_inputs
                && pt != tamper
            {
                g.log(format!("input: tamper {pt:#x} -> {tamper:#x}"));
            }
            if last_inputs.map(|(_, pt)| pt != 0) != Some(tamper != 0) {
                emit(Edge::Tamper(tamper != 0));
            }
            last_inputs = Some((inputs, tamper));
        }

        // Call button.
        while let Ok(pressed) = button_rx.try_recv() {
            g.log(format!(
                "button: {}",
                if pressed { "PRESS" } else { "release" }
            ));
            if pressed {
                g.st.lock().unwrap().button_presses += 1;
                match call.handle(
                    g.now_ms(),
                    Input::Pressed,
                    &call_limits(&reader),
                    fresh_id(),
                ) {
                    Ok(fx) => run_call(&mut call, fx),
                    Err(_) => g.log(format!("call: press ignored while {}", call.phase().name())),
                }
            }
            emit(Edge::CallButton(pressed));
        }

        // Remote commands.
        while let Some(command) = commands.as_ref().and_then(|c| c.try_next(Instant::now())) {
            let cmd = match command {
                mqtt::Command::App(Command::Door(cmd)) => cmd,
                mqtt::Command::App(Command::Call {
                    device,
                    call: id,
                    op,
                }) => {
                    let input = Input::Command {
                        device: &device,
                        call: id,
                        op,
                    };
                    match call.handle(g.now_ms(), input, &call_limits(&reader), fresh_id()) {
                        Ok(fx) => run_call(&mut call, fx),
                        Err(r) => g.log(format!(
                            "call: {op:?} from {} refused ({})",
                            device.as_str(),
                            r.name()
                        )),
                    }
                    continue;
                }
                mqtt::Command::Setting { slot, value } => {
                    let _ = settings_tx.send(live::Msg::Set { slot, value });
                    continue;
                }
                _ => continue,
            };
            if cmd == DoorCommand::Close {
                // Expire the pulse; the release above runs on the next pass with its retry logic.
                g.log("remote: close");
                g.st.lock().unwrap().strike.close_now();
                continue;
            }
            let quiet = cmd == DoorCommand::OpenQuiet;
            g.log(format!(
                "remote: open{}",
                if quiet { " (quiet)" } else { "" }
            ));
            if open(&g, "remote open") {
                announce("remote", "door_opened", quiet);
                emit(Edge::Remote);
            }
        }

        // Direct link (the panel).
        while let Some(event) = link_events.as_ref().and_then(|rx| rx.try_recv().ok()) {
            let limits = call_limits(&reader);
            match event {
                LinkEvent::Connected => {
                    // Fresh ages for the new client, also when the link count did not change.
                    call.bump();
                    publish_call(&call);
                }
                LinkEvent::Lost => {
                    g.log("call-link: panel lost");
                    if let Ok(fx) =
                        call.handle(g.now_ms(), Input::LeaseLost(&panel_id), &limits, fresh_id())
                    {
                        run_call(&mut call, fx);
                    }
                }
                LinkEvent::Command {
                    conn,
                    req,
                    call: id,
                    op,
                } => {
                    let input = Input::Command {
                        device: &panel_id,
                        call: id,
                        op,
                    };
                    let result = call
                        .handle(g.now_ms(), input, &limits, fresh_id())
                        .map(|fx| run_call(&mut call, fx));
                    if let Err(r) = result {
                        g.log(format!("call: {op:?} from panel refused ({})", r.name()));
                    }
                    if let Some(l) = call_link.as_ref() {
                        l.ack(conn, req, result);
                    }
                }
            }
        }
        // The snapshot also carries the link and the call settings: republish when they change.
        let context = (
            call_link.as_ref().is_some_and(|l| l.connected()),
            reader.get(live_settings::CALL_RING_TIMEOUT).unwrap_or(45),
            reader.get(live_settings::CALL_MAX_TALK).unwrap_or(3),
            reader
                .get(live_settings::CALL_PANEL_RING_VOLUME)
                .unwrap_or(80),
        );
        if context != context_published {
            context_published = context;
            call.bump();
            publish_call(&call);
        }
        let fx = call.tick(g.now_ms(), &call_limits(&reader));
        if !fx.is_empty() {
            run_call(&mut call, fx);
        }

        // Keypad.
        while let Ok(report) = keypad_rx.try_recv() {
            match report {
                keypad::Report::Phase(p) => {
                    g.st.lock().unwrap().keypad_phase = p;
                    g.log(format!("keypad: {p}"));
                    if p != "ready" {
                        access.reset();
                    }
                }
                keypad::Report::Text(text) => {
                    let released = g.st.lock().unwrap().relay_mask & STRIKE != 0;
                    match access.keypad(&text, now_s, released) {
                        Decision::None => {}
                        Decision::Grant { name, clip } => {
                            if !open(&g, &format!("PIN {name}")) {
                                g.log(format!("access: granted to {name}, but the strike failed"));
                                refuse();
                            } else {
                                g.st.lock().unwrap().granted += 1;
                                g.log(format!("access: granted to {name}"));
                                announce(&name, &clip, false);
                                emit(Edge::Granted(name.clone()));
                            }
                        }
                        Decision::Deny { failures } => {
                            g.st.lock().unwrap().denied += 1;
                            g.log(format!("access: denied, failure {failures}"));
                            refuse();
                            emit(Edge::Denied);
                        }
                    }
                }
            }
        }

        // Day/night from the sensor's exposure every 2 s.
        if last_ae.elapsed() >= Duration::from_secs(2) {
            last_ae = Instant::now();
            let (mut l, mut f, mut ag, mut dg, mut h) = (0u32, 0u32, 0u32, 0u32, 0u32);
            // SAFETY: five valid out-pointers; sensor register reads only.
            let rc = unsafe { gm_ae_state(&mut l, &mut f, &mut ag, &mut dg, &mut h) };
            if rc == 0 {
                let night = daynight.update(l, f, ag);
                let changed = {
                    let mut s = g.st.lock().unwrap();
                    let c = s.night != night;
                    s.night = night;
                    s.viewers = video.viewers() + sub.viewers();
                    c
                };
                if changed {
                    g.log(format!(
                        "image: AE says {}",
                        if night { "night" } else { "day" }
                    ));
                    let _ = settings_tx.send(live::Msg::Night(night));
                    set_ir(&g, if night { 60 } else { 0 });
                    set_call_light(&g, night);
                }
            }
        }

        // Boot health for the supervisor: media running and frames flowing after two minutes.
        if !health_written && g.t0.elapsed() >= HEALTHY_AFTER {
            let frames = g.st.lock().unwrap().stream_frames[0];
            if frames > 0 {
                match std::fs::write(&cfg.health, format!("{}\n", env!("CARGO_PKG_VERSION"))) {
                    Ok(()) => {
                        g.st.lock().unwrap().healthy = true;
                        g.log("health: boot marked good");
                    }
                    Err(e) => g.log(format!("health: cannot write marker: {e}")),
                }
                health_written = true;
            }
        }
        thread::sleep(Duration::from_millis(50));
    }

    g.log("ds-kd8003-ime1-cam: stopping");
    // The strike is released before anything that can block: a slow or stuck settings save must
    // never hold the door open.
    set_relay(&g, 0, "shutdown");
    // The settings thread saves anything unsaved when it sees the stop.
    drop(settings_tx);
    let _ = settings_thread.join();
    set_ir(&g, 0);
    // SAFETY: plain ioctl wrapper.
    unsafe { gm_amp(0) };
    thread::sleep(Duration::from_millis(500));
    std::process::exit(0);
}

/// ONVIF on `onvif_port` with the RTSP credentials; `None` when it is off or cannot start (logged: the
/// door works without it).
fn start_onvif(g: &Arc<Device>, cfg: &Config, jpeg: bool) -> Option<camera_onvif::Service> {
    if cfg.onvif_port == 0 {
        return None;
    }
    let identity = camera_onvif::identity(
        camera_onvif::Product {
            manufacturer: "Hikvision".into(),
            model: "DS-KD8003-IME1".into(),
            firmware: env!("CARGO_PKG_VERSION").into(),
            hardware: "DS-KD8003-IME1".into(),
            name: cfg.device_name.clone(),
            ntp: cfg.ntp.clone(),
            dhcp: true,
        },
        None,
    );
    let camera = Arc::new(onvif::DoorOnvif {
        identity,
        device: g.clone(),
        jpeg,
        buffer: Mutex::new(Vec::new()),
    });
    let creds = Credentials {
        user: cfg.rtsp_user.clone(),
        password: cfg.rtsp_password.clone(),
    };
    match camera_onvif::start(
        camera,
        creds,
        cfg.rtsp_port,
        cfg.onvif_port,
        camera_onvif::HttpLimits::DEFAULT,
        |line| eprintln!("{line}"),
    ) {
        Ok(service) => {
            g.log(format!("onvif: serving on {}", cfg.onvif_port));
            Some(service)
        }
        Err(e) => {
            g.log(format!("onvif: not started on {}: {e}", cfg.onvif_port));
            None
        }
    }
}

fn video_loop(g: Arc<Device>, chn: usize, hub: Arc<Hub>) {
    let mut buf = vec![0u8; AU_CAP];
    while !g.stop.load(Ordering::Relaxed) {
        if g.st.lock().unwrap().streams[chn].is_none() {
            thread::sleep(Duration::from_millis(200));
            continue;
        }
        let (mut len, mut pts, mut key) = (0u32, 0u64, 0i32);
        let (rc, fd) = {
            let _guard = g.video_lock[chn].lock().unwrap();
            // SAFETY: buf has AU_CAP bytes; the channel is live while its lock is held.
            unsafe {
                let rc = gm_video_chn(
                    chn as i32,
                    buf.as_mut_ptr(),
                    AU_CAP as u32,
                    &mut len,
                    &mut pts,
                    &mut key,
                );
                (rc, if rc == 1 { gm_video_fd(chn as i32) } else { -1 })
            }
        };
        match rc {
            0 => {
                let au = &buf[..len as usize];
                hub.publish(key != 0, pts, au.to_vec());
                let mut s = g.st.lock().unwrap();
                s.stream_frames[chn] += 1;
                s.stream_bytes[chn] += u64::from(len);
            }
            // Nothing ready: sleep on the encoder's descriptor (outside the channel lock, bounded so a
            // reconfigured channel is picked up) instead of asking again every 5 ms.
            1 if fd >= 0 => {
                let mut p = libc::pollfd {
                    fd,
                    events: libc::POLLIN,
                    revents: 0,
                };
                // SAFETY: one valid pollfd.
                let r = unsafe { libc::poll(&mut p, 1, 100) };
                if r < 0 || p.revents & (libc::POLLERR | libc::POLLNVAL) != 0 {
                    thread::sleep(Duration::from_millis(5));
                }
            }
            1 => thread::sleep(Duration::from_millis(5)),
            -9 => {
                g.log(format!("video {chn}: uncertain lease, stopped"));
                g.media_dead.store(true, Ordering::Relaxed);
                return;
            }
            _ => thread::sleep(Duration::from_millis(100)),
        }
    }
}

static STOP_FLAG: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP_FLAG.store(true, Ordering::Relaxed);
}

/// SIGTERM/SIGINT request an orderly stop; a thread mirrors the flag into the shared state.
fn install_signal_stop(g: &Arc<Device>) {
    // SAFETY: async-signal-safe handler that only stores an atomic.
    unsafe {
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
    }
    let g = g.clone();
    spawn_named("signals", move || {
        loop {
            if STOP_FLAG.load(Ordering::Relaxed) {
                g.stop.store(true, Ordering::Relaxed);
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
    });
}

/// Before an error exit: stops the settings thread, which saves anything unsaved, and waits for it
/// at most [`SAVE_BEFORE_EXIT`] so a stuck write cannot delay the reset.
fn save_settings(g: &Device, settings: &thread::JoinHandle<()>) {
    g.stop.store(true, Ordering::Relaxed);
    let until = Instant::now() + SAVE_BEFORE_EXIT;
    while !settings.is_finished() && Instant::now() < until {
        thread::sleep(Duration::from_millis(20));
    }
}

const SAVE_BEFORE_EXIT: Duration = Duration::from_secs(2);

/// Four bytes from the kernel's generator; the clock when it is unreadable (ids need freshness only).
fn random4() -> [u8; 4] {
    use std::io::Read;
    let mut b = [0u8; 4];
    if std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .is_err()
    {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        b = t.to_le_bytes();
    }
    b
}

/// The call's limits from the live settings (read at each pass, so a change applies at once).
fn call_limits(reader: &camera_transport::settings::Reader) -> Limits {
    Limits {
        ring_timeout_ms: reader
            .get(live_settings::CALL_RING_TIMEOUT)
            .unwrap_or(45)
            .max(1) as u64
            * 1000,
        max_talk_ms: reader.get(live_settings::CALL_MAX_TALK).unwrap_or(3).max(1) as u64 * 60_000,
        card_lease_ms: 30_000,
    }
}

/// Writes the missed mark through a temporary file, so a crash leaves the old one or the new one.
fn save_missed(g: &Device, path: &std::path::Path, missed: Option<&call::Missed>) {
    let tmp = path.with_extension("json.tmp");
    let r = std::fs::write(&tmp, call_wire::missed_file(missed))
        .and_then(|_| std::fs::rename(&tmp, path));
    if let Err(e) = r {
        g.log(format!("call: cannot save {}: {e}", path.display()));
    }
}
