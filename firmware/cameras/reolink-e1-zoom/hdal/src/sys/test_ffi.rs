//! Test-only C-call boundary. Real Sys code runs against per-thread driver outcomes.
pub use hdal_sys::*;
use std::cell::RefCell;
#[derive(Default)]
pub struct Driver {
    pub session: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub fail_at: Option<usize>,
    pub swallowed_ioctl: Option<&'static str>,
    pub swallowed_errors: Vec<i32>,
    pub ioctl_command: Option<usize>,
    pub calls: Vec<&'static str>,
    pub errors: std::collections::HashMap<&'static str, i32>,
    pub bytes: Vec<u8>,
    pub video: HD_VIDEOENC_BS,
    pub audio: HD_AUDIO_FRAME,
    pub map_result: Option<usize>,
    pub phy: u32,
    pub size: u32,
    /// The last `HD_AUDIOCAP_PARAM_VOLUME` set.
    pub cap_volume: Option<u32>,
}
thread_local! { pub static DRIVER: RefCell<Driver> = RefCell::new(Driver::default()); }
pub fn call(name: &'static str) -> i32 {
    DRIVER.with_borrow_mut(|d| {
        d.calls.push(name);
        if d.swallowed_ioctl == Some(name) {
            for error in if d.swallowed_errors.is_empty() {
                &[14][..]
            } else {
                &d.swallowed_errors
            } {
                super::ioctl_capture::record(
                    -1,
                    *error,
                    d.ioctl_command.unwrap_or(0xc0144905),
                    Some(0x1020),
                );
            }
            super::ioctl_capture::record(0, 99, 0xc0144905, Some(0x1021));
        }
        if d.fail_at == Some(d.calls.len()) {
            -77
        } else {
            *d.errors.get(name).unwrap_or(&0)
        }
    })
}
macro_rules! stub {
    ($name:ident($($arg:ident: $ty:ty),* $(,)?) -> $ret:ty) => {
        pub unsafe extern "C" fn $name($($arg: $ty),*) -> $ret {
            let _ = ($($arg,)*);
            call(stringify!($name)) as $ret
        }
    };
}
stub!(hd_audiocap_close(path_id: HD_PATH_ID) -> HD_RESULT);
stub!(hd_audiocap_init() -> HD_RESULT);
stub!(
    hd_audiocap_open(in_id: HD_IN_ID, out_id: HD_OUT_ID, p_path_id: *mut HD_PATH_ID) -> HD_RESULT
);
pub unsafe extern "C" fn hd_audiocap_set(
    _: HD_PATH_ID,
    id: HD_AUDIOCAP_PARAM_ID,
    p_param: *mut VOID,
) -> HD_RESULT {
    let rc = call("hd_audiocap_set");
    if id == HD_AUDIOCAP_PARAM_VOLUME {
        // SAFETY: Sys passes an HD_AUDIOCAP_VOLUME for the VOLUME id.
        let volume = unsafe { (*p_param.cast::<HD_AUDIOCAP_VOLUME>()).volume };
        DRIVER.with_borrow_mut(|d| d.cap_volume = Some(volume));
    }
    rc as HD_RESULT
}
stub!(hd_audiocap_release_out_buf(path_id: HD_PATH_ID, p_audio_frame: *mut HD_AUDIO_FRAME) -> HD_RESULT);
stub!(hd_audiocap_start(path_id: HD_PATH_ID) -> HD_RESULT);
stub!(hd_audiocap_stop(path_id: HD_PATH_ID) -> HD_RESULT);
stub!(hd_audiocap_uninit() -> HD_RESULT);
stub!(hd_audioout_close(path_id: HD_PATH_ID) -> HD_RESULT);
stub!(hd_audioout_init() -> HD_RESULT);
stub!(
    hd_audioout_open(in_id: HD_IN_ID, out_id: HD_OUT_ID, p_path_id: *mut HD_PATH_ID) -> HD_RESULT
);
stub!(
    hd_audioout_push_in_buf(
        path_id: HD_PATH_ID,
        p_in_audio_frame: *mut HD_AUDIO_FRAME,
        wait_ms: INT32,
    ) -> HD_RESULT
);
stub!(
    hd_audioout_set(path_id: HD_PATH_ID, id: HD_AUDIOOUT_PARAM_ID, p_param: *mut VOID) -> HD_RESULT
);
stub!(hd_audioout_start(path_id: HD_PATH_ID) -> HD_RESULT);
stub!(hd_audioout_stop(path_id: HD_PATH_ID) -> HD_RESULT);
stub!(hd_audioout_uninit() -> HD_RESULT);
stub!(hd_common_init(sys_config_type: UINT32) -> HD_RESULT);
stub!(hd_common_mem_flush_cache(virt_addr: *mut ctypes::c_void, size: ctypes::c_uint) -> HD_RESULT);
stub!(hd_common_mem_free(phy_addr: UINTPTR, virt_addr: *mut ctypes::c_void) -> HD_RESULT);
stub!(hd_common_mem_init(p_mem_config: *mut HD_COMMON_MEM_INIT_CONFIG) -> HD_RESULT);
stub!(hd_common_mem_munmap(virt_addr: *mut ctypes::c_void, size: ctypes::c_uint) -> HD_RESULT);
stub!(hd_common_mem_uninit() -> HD_RESULT);
stub!(hd_common_uninit() -> HD_RESULT);
stub!(hd_videocap_bind(out_id: HD_OUT_ID, dest_in_id: HD_IN_ID) -> HD_RESULT);
stub!(hd_videocap_close(path_id: HD_PATH_ID) -> HD_RESULT);
stub!(
    hd_videocap_get(path_id: HD_PATH_ID, id: HD_VIDEOCAP_PARAM_ID, p_param: *mut VOID) -> HD_RESULT
);
stub!(hd_videocap_init() -> HD_RESULT);
stub!(
    hd_videocap_open(in_id: HD_IN_ID, out_id: HD_OUT_ID, p_path_id: *mut HD_PATH_ID) -> HD_RESULT
);
stub!(
    hd_videocap_set(path_id: HD_PATH_ID, id: HD_VIDEOCAP_PARAM_ID, p_param: *mut VOID) -> HD_RESULT
);
stub!(hd_videocap_start(path_id: HD_PATH_ID) -> HD_RESULT);
stub!(hd_videocap_stop(path_id: HD_PATH_ID) -> HD_RESULT);
stub!(hd_videocap_unbind(out_id: HD_OUT_ID) -> HD_RESULT);
stub!(hd_videocap_uninit() -> HD_RESULT);
stub!(hd_videoenc_close(path_id: HD_PATH_ID) -> HD_RESULT);
stub!(hd_videoenc_init() -> HD_RESULT);
stub!(
    hd_videoenc_open(in_id: HD_IN_ID, out_id: HD_OUT_ID, p_path_id: *mut HD_PATH_ID) -> HD_RESULT
);
stub!(
    hd_videoenc_release_out_buf(
        path_id: HD_PATH_ID,
        p_videoenc_bs: *mut HD_VIDEOENC_BS,
    ) -> HD_RESULT
);
stub!(
    hd_videoenc_set(path_id: HD_PATH_ID, id: HD_VIDEOENC_PARAM_ID, p_param: *mut VOID) -> HD_RESULT
);
stub!(hd_videoenc_start(path_id: HD_PATH_ID) -> HD_RESULT);
stub!(hd_videoenc_stop(path_id: HD_PATH_ID) -> HD_RESULT);
stub!(hd_videoenc_uninit() -> HD_RESULT);
stub!(hd_videoproc_bind(out_id: HD_OUT_ID, dest_in_id: HD_IN_ID) -> HD_RESULT);
stub!(hd_videoproc_close(path_id: HD_PATH_ID) -> HD_RESULT);
stub!(hd_videoproc_init() -> HD_RESULT);
stub!(
    hd_videoproc_open(in_id: HD_IN_ID, out_id: HD_OUT_ID, p_path_id: *mut HD_PATH_ID) -> HD_RESULT
);
stub!(
    hd_videoproc_set(
        path_id: HD_PATH_ID,
        id: HD_VIDEOPROC_PARAM_ID,
        p_param: *mut VOID,
    ) -> HD_RESULT
);
stub!(hd_videoproc_start(path_id: HD_PATH_ID) -> HD_RESULT);
stub!(hd_videoproc_stop(path_id: HD_PATH_ID) -> HD_RESULT);
stub!(hd_videoproc_unbind(out_id: HD_OUT_ID) -> HD_RESULT);
stub!(hd_videoproc_uninit() -> HD_RESULT);

pub unsafe extern "C" fn hd_common_mem_mmap(_: u32, _: UINTPTR, _: u32) -> *mut ctypes::c_void {
    call("hd_common_mem_mmap");
    DRIVER.with_borrow_mut(|d| {
        d.map_result
            .map_or(d.bytes.as_mut_ptr(), |p| p as *mut u8)
            .cast()
    })
}
pub unsafe extern "C" fn hd_videoenc_get(_: u32, _: u32, info: *mut ctypes::c_void) -> i32 {
    let rc = call("hd_videoenc_get");
    // SAFETY: Sys supplies the BUFINFO output allocation.
    unsafe {
        DRIVER.with_borrow(|d| {
            let i = &mut *info.cast::<HD_VIDEOENC_BUFINFO>();
            i.buf_info.phy_addr = d.phy as _;
            i.buf_info.buf_size = d.size;
        });
    }
    rc
}
pub unsafe extern "C" fn hd_audiocap_get(_: u32, _: u32, info: *mut ctypes::c_void) -> i32 {
    let rc = call("hd_audiocap_get");
    // SAFETY: Sys supplies the BUFINFO output allocation.
    unsafe {
        DRIVER.with_borrow(|d| {
            let i = &mut *info.cast::<HD_AUDIOCAP_BUFINFO>();
            i.buf_info.phy_addr = d.phy as _;
            i.buf_info.buf_size = d.size;
        });
    }
    rc
}
pub unsafe extern "C" fn hd_videoenc_poll_list(
    p: *mut HD_VIDEOENC_POLL_LIST,
    n: u32,
    _: i32,
) -> i32 {
    // SAFETY: Sys supplies n initialized poll entries.
    unsafe {
        for e in std::slice::from_raw_parts_mut(p, n as usize) {
            e.revent.event = 1;
        }
    }
    call("hd_videoenc_poll_list")
}
pub unsafe extern "C" fn hd_videoenc_pull_out_buf(_: u32, bs: *mut HD_VIDEOENC_BS, _: i32) -> i32 {
    // SAFETY: Sys supplies a writable descriptor.
    unsafe {
        *bs = DRIVER.with_borrow(|d| d.video);
    }
    call("hd_videoenc_pull_out_buf")
}
pub unsafe extern "C" fn hd_audiocap_pull_out_buf(
    _: u32,
    frame: *mut HD_AUDIO_FRAME,
    _: i32,
) -> i32 {
    // SAFETY: Sys supplies a writable descriptor.
    unsafe {
        *frame = DRIVER.with_borrow(|d| d.audio);
    }
    call("hd_audiocap_pull_out_buf")
}
pub unsafe extern "C" fn hd_common_mem_alloc(
    _: *mut ctypes::c_char,
    phy: *mut UINTPTR,
    va: *mut *mut ctypes::c_void,
    _: u32,
    _: u32,
) -> i32 {
    let rc = call("hd_common_mem_alloc");
    if rc == 0 {
        // SAFETY: Sys supplies writable out pointers; the fixture owns the bytes for the test lifetime.
        unsafe {
            DRIVER.with_borrow_mut(|d| {
                *phy = d.phy as _;
                *va = d
                    .map_result
                    .map_or(d.bytes.as_mut_ptr(), |p| p as *mut u8)
                    .cast();
            });
        }
    }
    rc
}
