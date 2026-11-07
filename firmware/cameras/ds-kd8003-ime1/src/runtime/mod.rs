//! The application on the board: stops the vendor processes, owns media and I/O, serves RTSP, runs
//! the keypad and PIN policy, talks MQTT, feeds the watchdog only while its own loop is alive.
mod audio;
mod ffi;
mod http;
mod hw;
mod keypad;
mod link;
mod uart;

use camera_transport::microphone::Format;
use camera_transport::{AudioHub, Config as RtspConfig, Credentials, Hub};
use ds_kd8003_ime1_cam::access::{Access, Decision};
use ds_kd8003_ime1_cam::config::{self, Config};
use ds_kd8003_ime1_cam::discovery::{self, DoorCommand};
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
/// The strike never stays energized longer than this whatever the configuration says.
const MAX_PULSE_MS: u64 = 120_000;
/// Healthy for this long after start marks the boot good for the supervisor.
const HEALTHY_AFTER: Duration = Duration::from_secs(120);
/// After this many consecutive failed strike releases (one per 50 ms pass) the process exits so the unfed watchdog resets the board.
const MAX_RELEASE_FAILURES: u32 = 20;
/// Clip played instead of the built-in tone when a code is refused, if the payload ships it.
const DENIED_CLIP: &str = "denied";
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
    pub relay_until_ms: Option<u64>,
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
    let cfg = match Config::load(std::path::Path::new(&args[3])) {
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
    g.log(format!(
        "ds-kd8003-ime1-cam {}: start, {} PINs, open {} s",
        env!("CARGO_PKG_VERSION"),
        credentials.len(),
        cfg.open_seconds
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
    set_isp(&g, 8, 1); // AWB auto: without it the image is magenta
    // Sub stream as the stock firmware offered it: 704x480 at 15 fps.
    set_stream(
        &g,
        1,
        &StreamSettings {
            w: 704,
            h: 480,
            fps: 15,
            kbps: 1024,
            gop: 30,
            vbr: true,
            profile: 1,
        },
    );
    // SAFETY: plain acodec ioctl wrappers.
    unsafe {
        gm_codec_set(1, cfg.codec_out_db);
        gm_codec_set(0, cfg.mic_gain_db);
    }
    g.beat();

    let video = Hub::new();
    let sub = Hub::new();
    let mic = AudioHub::with_format(Format::Pcmu8k);
    for (chn, hub) in [(0usize, video.clone()), (1usize, sub.clone())] {
        let g2 = g.clone();
        spawn_named(&format!("video{chn}"), move || video_loop(g2, chn, hub));
    }
    {
        let (g2, hub) = (g.clone(), mic.clone());
        spawn_named("mic", move || audio::capture_loop(g2, hub));
    }
    // Talkback: the shared Opus backchannel mixer (same crate and decoder as the other cameras),
    // decoding straight at the speaker's rate.
    let talk = camera_opus::talkback(audio::codec_rate());
    match camera_transport::bind(cfg.rtsp_port) {
        Ok(listener) => {
            let rtsp = Arc::new(RtspConfig {
                streams: vec![
                    ("Streaming/Channels/101".to_string(), video.clone()),
                    ("Streaming/Channels/102".to_string(), sub.clone()),
                    ("main".to_string(), video.clone()),
                    ("sub".to_string(), sub.clone()),
                ],
                creds: Some(Credentials {
                    user: cfg.rtsp_user.clone(),
                    password: cfg.rtsp_password.clone(),
                }),
                audio: Some(mic.clone()),
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

    let (speaker_tx, speaker_rx) = mpsc::channel();
    let clips = audio::load_clips(&g, &cfg.clips);
    if ao_rc == 0 {
        let g2 = g.clone();
        let db = cfg.clip_db;
        let talk_db = cfg.talk_db;
        let talk = Arc::clone(&talk);
        spawn_named("speaker", move || {
            audio::speaker_loop(g2, speaker_rx, db, talk_db, talk)
        });
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

    let (command_tx, command_rx) = mpsc::channel::<DoorCommand>();
    let (publisher, _mqtt) = link::start(g.clone(), &cfg, command_tx);
    let topics = discovery::Topics::new(&cfg.mqtt_prefix, &cfg.mqtt_node);
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

    let mut access = Access::new(credentials);
    let open_ms = u64::from(cfg.open_seconds) * 1000;
    let mut last_inputs: Option<(u32, u32)> = None;
    let mut last_ae = Instant::now() - Duration::from_secs(5);
    let mut daynight = DayNight::default();
    let mut unlocked_published: Option<bool> = None;
    let mut cover_published: Option<bool> = None;
    let mut health_written = false;
    g.st.lock().unwrap().phase = "running".into();
    g.log("ds-kd8003-ime1-cam: running");

    let announce = |name: &str, clip: &str, quiet: bool| {
        if quiet {
            return;
        }
        match clips.get(clip).or_else(|| clips.get("door_opened")) {
            Some(samples) => {
                let _ = speaker_tx.send(audio::Sound::Clip(clip.to_string(), samples.clone()));
            }
            None => g.log(format!("speaker: no clip for {name}")),
        }
    };
    let refuse = || {
        let _ = speaker_tx.send(match clips.get(DENIED_CLIP) {
            Some(samples) => audio::Sound::Clip(DENIED_CLIP.to_string(), samples.clone()),
            None => audio::Sound::Deny,
        });
    };
    let open = |g: &Device, reason: &str| {
        let rc = set_relay(g, STRIKE, reason);
        let mut s = g.st.lock().unwrap();
        if rc == 0 {
            s.relay_until_ms = Some(g.now_ms() + open_ms.min(MAX_PULSE_MS));
            s.relay_pulses += 1;
        } else {
            // The output may have switched anyway: release it on the next pass.
            s.relay_until_ms = Some(0);
        }
        rc == 0
    };

    while !g.stop.load(Ordering::Relaxed) {
        let now = g.now_ms();
        let now_s = now / 1000;
        g.beat();

        // Strike pulse end. A failed release is retried every pass; the recorded mask only changes on
        // success, so `expired` stays true until the hardware confirms the strike is off.
        let expired = {
            let s = g.st.lock().unwrap();
            s.relay_mask != 0 && s.relay_until_ms.map(|u| now >= u).unwrap_or(true)
        };
        if expired && set_relay(&g, 0, "pulse end") == 0 {
            g.st.lock().unwrap().release_failures = 0;
        } else if expired {
            let failures = {
                let mut s = g.st.lock().unwrap();
                s.release_failures += 1;
                s.release_failures
            };
            if failures >= MAX_RELEASE_FAILURES {
                g.log("relay: cannot release the strike, exiting so the watchdog resets the board");
                std::process::exit(6);
            }
        }
        if g.media_dead.load(Ordering::Relaxed) && g.st.lock().unwrap().relay_mask == 0 {
            g.log("media: lease lost, exiting for a reboot");
            std::process::exit(7);
        }
        let unlocked = g.st.lock().unwrap().relay_mask & STRIKE != 0;
        if unlocked_published != Some(unlocked) {
            publisher.publish(&topics.lock_state, discovery::lock_state(unlocked), true);
            unlocked_published = Some(unlocked);
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
                publisher.publish(&topics.reed, discovery::door_state(reed_open), true);
            }
            // The cover is "open" while the strike is released or the leaf is away from the reed:
            // Home Assistant only offers "close" on an open cover, and a released strike is what
            // "close now" ends.
            let cover_open = inputs & 1 != 0 || unlocked;
            if cover_published != Some(cover_open) {
                publisher.publish(&topics.door_state, discovery::door_state(cover_open), true);
                cover_published = Some(cover_open);
            }
            if let Some((_, pt)) = last_inputs
                && pt != tamper
            {
                g.log(format!("input: tamper {pt:#x} -> {tamper:#x}"));
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
            }
            publisher.publish(
                &topics.call_button,
                discovery::event(if pressed { "press" } else { "release" }, None),
                false,
            );
        }

        // Remote commands.
        while let Ok(cmd) = command_rx.try_recv() {
            if cmd == DoorCommand::Close {
                // Expire the pulse; the release above runs on the next pass with its retry logic.
                g.log("remote: close");
                g.st.lock().unwrap().relay_until_ms = Some(0);
                continue;
            }
            let quiet = cmd == DoorCommand::OpenQuiet;
            g.log(format!(
                "remote: open{}",
                if quiet { " (quiet)" } else { "" }
            ));
            if open(&g, "remote open") {
                announce("remote", "door_opened", quiet);
                publisher.publish(&topics.access, discovery::event("remote", None), false);
            }
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
                                publisher.publish(
                                    &topics.access,
                                    discovery::event("granted", Some(&name)),
                                    false,
                                );
                            }
                        }
                        Decision::Deny { failures } => {
                            g.st.lock().unwrap().denied += 1;
                            g.log(format!("access: denied, failure {failures}"));
                            refuse();
                            publisher.publish(
                                &topics.access,
                                discovery::event("denied", None),
                                false,
                            );
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
                    apply_color(&g);
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
    set_relay(&g, 0, "shutdown");
    set_ir(&g, 0);
    // SAFETY: plain ioctl wrapper.
    unsafe { gm_amp(0) };
    thread::sleep(Duration::from_millis(500));
    std::process::exit(0);
}

fn video_loop(g: Arc<Device>, chn: usize, hub: Arc<Hub>) {
    let mut buf = vec![0u8; AU_CAP];
    while !g.stop.load(Ordering::Relaxed) {
        if g.st.lock().unwrap().streams[chn].is_none() {
            thread::sleep(Duration::from_millis(200));
            continue;
        }
        let (mut len, mut pts) = (0u32, 0u64);
        let rc = {
            let _guard = g.video_lock[chn].lock().unwrap();
            // SAFETY: buf has AU_CAP bytes; the channel is live while its lock is held.
            unsafe {
                gm_video_chn(
                    chn as i32,
                    buf.as_mut_ptr(),
                    AU_CAP as u32,
                    &mut len,
                    &mut pts,
                )
            }
        };
        match rc {
            0 => {
                let au = &buf[..len as usize];
                let key = au
                    .windows(4)
                    .any(|w| w[0] == 0 && w[1] == 0 && w[2] == 1 && (w[3] & 31) == 5);
                hub.publish(key, pts, au.to_vec());
                g.st.lock().unwrap().stream_frames[chn] += 1;
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
