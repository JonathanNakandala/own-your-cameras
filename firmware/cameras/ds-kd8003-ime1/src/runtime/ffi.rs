//! The C media bridge (`native/media_bridge.c`) and the few raw device helpers.
use std::ffi::{CString, c_char};

unsafe extern "C" {
    pub fn gm_start(dir: *const c_char) -> i32;
    pub fn gm_audio(pcm: *mut u8, cap: u32, len: *mut u32, pts: *mut u64) -> i32;
    pub fn gm_saturation(level: i32) -> i32;
    pub fn gm_ae_state(
        lines: *mut u32,
        fll: *mut u32,
        again: *mut u32,
        dgain: *mut u32,
        hcg: *mut u32,
    ) -> i32;
    pub fn gm_ao_start() -> i32;
    pub fn gm_audio_rate(rate: u32) -> i32;
    pub fn gm_ao_send(pcm: *const u8, len: u32) -> i32;
    pub fn gm_amp(on: i32) -> i32;
    pub fn gm_codec_set(which: i32, db: i32) -> i32;
    pub fn gm_isp(kind: i32, value: i32) -> i32;
    pub fn gm_video_chn(
        chn: i32,
        buf: *mut u8,
        cap: u32,
        len: *mut u32,
        pts: *mut u64,
        key: *mut i32,
    ) -> i32;
    pub fn gm_video_fd(chn: i32) -> i32;
    pub fn gm_stream_set(
        chn: i32,
        w: u32,
        h: u32,
        fps: u32,
        kbps: u32,
        gop: u32,
        vbr: i32,
        profile: u32,
    ) -> i32;
    pub fn gm_stream_get(chn: i32, out: *mut u32) -> i32;
}

pub const O_RDWR: i32 = 2;
pub const O_RDONLY: i32 = 0;
const O_CLOEXEC: i32 = 0o2000000;

pub fn open_dev(path: &str, flags: i32) -> i32 {
    let c = CString::new(path).unwrap();
    // SAFETY: valid NUL-terminated path.
    unsafe { libc::open(c.as_ptr(), flags | O_CLOEXEC) }
}

pub fn ioctl_buf(fd: i32, req: u32, buf: &mut [u8]) -> i32 {
    if fd < 0 {
        return -100;
    }
    // SAFETY: buf is live and at least as large as the driver copies for these requests.
    unsafe { libc::ioctl(fd, req as libc::c_ulong, buf.as_mut_ptr()) }
}

pub fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

pub fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 32 => o.push(' '),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}
