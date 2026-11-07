//! Board I/O: vendor stop, strike relay, LEDs, IR, image controls, encoder settings.
use super::Device;
use super::ffi::*;
use std::thread;
use std::time::{Duration, Instant};

pub const ALARM_INPUTS: u32 = 0x8004_4201;
pub const ALARM_TAMPER: u32 = 0x4004_4204;
const ALARM_OUTPUTS: u32 = 0x4004_4203;
const IR_DUTY: u32 = 0x4001_4e00;
/// `/dev/DDM/led`: 20-byte request {selector u8, pad, function u32, on u32, …}.
const LED_SET: u32 = 0x4014_4c01;
pub const ISP_NAMES: [&str; 9] = [
    "brightness",
    "contrast",
    "saturation",
    "sharpness",
    "denoise",
    "defog_on",
    "defog",
    "antiflicker",
    "awb",
];
/// Strike relay = COM1 = bit 0 of the alarm output mask.
pub const STRIKE: u32 = 1;

fn proc_field(pid: u32, file: &str) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/{file}")).ok()
}
fn ppid(pid: u32) -> Option<u32> {
    proc_field(pid, "status")?
        .lines()
        .find(|l| l.starts_with("PPid:"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}
fn alive(pid: u32) -> bool {
    proc_field(pid, "stat")
        .map(|s| !s.contains(") Z "))
        .unwrap_or(false)
}
fn processes() -> Vec<(u32, String)> {
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir("/proc") {
        for e in rd.flatten() {
            if let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok())
                && let Some(comm) = proc_field(pid, "comm")
            {
                v.push((pid, comm.trim().to_string()));
            }
        }
    }
    v
}

/// Processes that own the hardware on the stock image.
const VENDOR: [&str; 5] = [
    "manager",
    "daemon_fsp_app",
    "hicore",
    "sipServer",
    "gpl_process",
];

/// TERM the vendor supervisor shell and the vendor owners of the hardware, KILL survivors after 8 s.
pub fn stop_vendors(g: &Device) {
    let me = std::process::id();
    let procs = processes();
    let mut targets: Vec<(u32, String)> = Vec::new();
    for (pid, comm) in &procs {
        if comm == "sh"
            && ppid(*pid) == Some(1)
            && procs
                .iter()
                .any(|(c, n)| n == "manager" && ppid(*c) == Some(*pid))
        {
            targets.push((*pid, "sh(supervisor)".into()));
        }
    }
    for name in VENDOR {
        for (pid, comm) in &procs {
            if comm == name && *pid != me {
                targets.push((*pid, comm.clone()));
            }
        }
    }
    for (pid, name) in &targets {
        // SAFETY: signal to a pid just observed with a known vendor comm.
        let rc = unsafe { libc::kill(*pid as i32, libc::SIGTERM) };
        g.log(format!("stop: TERM {name} pid {pid} rc {rc}"));
        thread::sleep(Duration::from_millis(150));
    }
    let end = Instant::now() + Duration::from_secs(8);
    loop {
        g.beat();
        let left: Vec<&(u32, String)> = targets.iter().filter(|(p, _)| alive(*p)).collect();
        if left.is_empty() || Instant::now() > end {
            for (pid, name) in left {
                // SAFETY: same observed pid, still alive after the grace period.
                let rc = unsafe { libc::kill(*pid as i32, libc::SIGKILL) };
                g.log(format!("stop: KILL {name} pid {pid} rc {rc}"));
            }
            break;
        }
        thread::sleep(Duration::from_millis(200));
    }
    thread::sleep(Duration::from_millis(1500));
}

/// Drives the alarm outputs. A failed release leaves the recorded mask unchanged, so the caller keeps
/// retrying until the hardware confirms it. A failed energize may still have switched the output, so
/// those bits are recorded as possibly on and the caller schedules their release.
pub fn set_relay(g: &Device, mask: u32, reason: &str) -> i32 {
    let mut m = mask.to_le_bytes();
    let rc = ioctl_buf(g.alarm_fd, ALARM_OUTPUTS, &mut m);
    let err = if rc == 0 { 0 } else { errno() };
    {
        let mut s = g.st.lock().unwrap();
        s.relay_rc = rc;
        if rc == 0 {
            s.relay_mask = mask;
            if mask == 0 {
                s.relay_until_ms = None;
            }
        } else {
            s.relay_mask |= mask;
        }
    }
    g.log(format!(
        "relay: mask {mask:#04b} ({reason}) rc {rc} errno {err}"
    ));
    rc
}

/// The call-button light (LED0, GPIO 29): on through the night like the keypad backlight, off by day.
pub fn set_call_light(g: &Device, on: bool) -> i32 {
    let mut req = [0u8; 20];
    req[4..8].copy_from_slice(&4u32.to_le_bytes());
    req[8..12].copy_from_slice(&u32::from(on).to_le_bytes());
    let rc = ioctl_buf(g.led_fd, LED_SET, &mut req);
    g.log(format!(
        "light: call button {} rc {rc}",
        if on { "on" } else { "off" }
    ));
    rc
}

pub fn set_ir(g: &Device, duty: u8) -> i32 {
    let mut d = [duty.min(100)];
    let rc = ioctl_buf(g.ir_fd, IR_DUTY, &mut d);
    g.st.lock().unwrap().ir_duty = d[0];
    g.log(format!("ir: duty {}% rc {rc}", d[0]));
    rc
}

/// Colour by day, monochrome by night (the AE readings decide).
pub fn apply_color(g: &Device) {
    let night = g.st.lock().unwrap().night;
    // SAFETY: plain value call into the stock ISP context owned by the bridge.
    let rc = unsafe { gm_saturation(if night { 0 } else { 50 }) };
    g.log(format!(
        "image: {} rc {rc}",
        if night { "monochrome" } else { "colour" }
    ));
}

pub fn set_isp(g: &Device, kind: usize, value: i32) -> i32 {
    // SAFETY: bridge validates kind and the live ISP context.
    let rc = unsafe { gm_isp(kind as i32, value) };
    g.log(format!("image: {} = {value} rc {rc}", ISP_NAMES[kind]));
    rc
}

pub fn refresh_stream(g: &Device, chn: usize) {
    let mut out = [0u32; 7];
    // SAFETY: out has 7 slots.
    let rc = unsafe { gm_stream_get(chn as i32, out.as_mut_ptr()) };
    g.st.lock().unwrap().streams[chn] = if rc == 0 { Some(out) } else { None };
}

/// Encoder settings for one channel.
#[derive(Clone, Copy)]
pub struct StreamSettings {
    pub w: u32,
    pub h: u32,
    pub fps: u32,
    pub kbps: u32,
    pub gop: u32,
    pub vbr: bool,
    /// H.264 profile index as the bridge numbers it.
    pub profile: u32,
}

/// Reconfigure one encoder channel (0 main, 1 sub) with its video lock held.
pub fn set_stream(g: &Device, chn: usize, s: &StreamSettings) -> i32 {
    let StreamSettings {
        w,
        h,
        fps,
        kbps,
        gop,
        vbr,
        profile,
    } = *s;
    let rc = {
        let _guard = g.video_lock[chn].lock().unwrap();
        g.st.lock().unwrap().streams[chn] = None;
        // SAFETY: no reader touches this channel while its lock is held.
        unsafe { gm_stream_set(chn as i32, w, h, fps, kbps, gop, vbr as i32, profile) }
    };
    refresh_stream(g, chn);
    g.log(format!(
        "stream {}: {w}x{h} {fps} fps {kbps} kbit/s gop {gop} {} profile {profile} rc {rc}",
        if chn == 0 { "main" } else { "sub" },
        if vbr { "VBR" } else { "CBR" }
    ));
    rc
}

/// Same hysteresis as the qualified media core.
#[derive(Default)]
pub struct DayNight {
    night: bool,
    streak: u32,
}
impl DayNight {
    pub fn update(&mut self, lines: u32, frame_lines: u32, again: u32) -> bool {
        let coarse = (again >> 4) & 3;
        let dark = frame_lines > 0 && lines + 16 >= frame_lines && coarse >= 2;
        let bright = frame_lines > 0 && lines * 2 < frame_lines && coarse <= 1;
        if (!self.night && dark) || (self.night && bright) {
            self.streak += 1;
            if self.streak >= 3 {
                self.night = !self.night;
                self.streak = 0;
            }
        } else {
            self.streak = 0;
        }
        self.night
    }
}
