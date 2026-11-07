// Maintained HDAL 2.00 interface for the qualified E1 Zoom.
// Declarations only; implementations remain in the camera runtime.

pub const HD_VIDEO_PIX_RGGB_B: u32 = 259;

pub const HD_VIDEO_DIR_NONE: u32 = 0;

pub const VENC_BS_PACK_MAX: u32 = 4;

pub const HD_AUDIO_MAX_CH: u32 = 2;

pub const HD_VIDEOCAP_SEN_NAME_LEN: u32 = 32;

pub const HD_VIDEOCAP_SEN_IGNORE: u32 = 4294967295;

pub const HD_VIDEOCAP_SEN_MODE_AUTO: u32 = 4294967295;

pub type VOID = crate::ctypes::c_void;

pub type BOOLEAN = crate::ctypes::c_int;

pub type UINT64 = crate::ctypes::c_ulonglong;

pub type INT64 = crate::ctypes::c_longlong;

pub type UINT32 = crate::ctypes::c_uint;

pub type INT32 = crate::ctypes::c_int;

pub type UINT16 = crate::ctypes::c_ushort;

pub type INT16 = crate::ctypes::c_short;

pub type UINT8 = crate::ctypes::c_uchar;

pub type INT8 = crate::ctypes::c_schar;

pub type INT = crate::ctypes::c_int;

pub type DOUBLE = f64;

pub type CHAR = crate::ctypes::c_char;

pub type BOOL = crate::ctypes::c_uint;

pub type UINTPTR = usize;

pub type INTPTR = isize;

pub type ULONG = crate::ctypes::c_ulong;

pub type LONG = crate::ctypes::c_long;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_UPOINT {
    pub x: UINT32,
    pub y: UINT32,
}

pub type HD_UPOINT = _HD_UPOINT;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_URECT {
    pub x: UINT32,
    pub y: UINT32,
    pub w: UINT32,
    pub h: UINT32,
}

pub type HD_URECT = _HD_URECT;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_IPOINT {
    pub x: INT32,
    pub y: INT32,
}

pub type HD_IPOINT = _HD_IPOINT;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_IRECT {
    pub x: INT32,
    pub y: INT32,
    pub w: INT32,
    pub h: INT32,
}

pub type HD_IRECT = _HD_IRECT;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_BUFINFO {
    pub phy_addr: UINTPTR,
    pub buf_size: UINT32,
}

pub type HD_BUFINFO = _HD_BUFINFO;

pub const HD_ERR_TIMEDOUT: _HD_RESULT = -15;

pub const HD_ERR_INIT: _HD_RESULT = -20;

pub const HD_ERR_STATE: _HD_RESULT = -22;

pub const HD_ERR_TERM: _HD_RESULT = -23;

pub const HD_ERR_INV: _HD_RESULT = -35;

pub const HD_ERR_IN_USE: _HD_RESULT = -44;

pub const HD_ERR_OVERRUN: _HD_RESULT = -55;

pub const HD_ERR_UNDERRUN: _HD_RESULT = -56;

pub type _HD_RESULT = crate::ctypes::c_int;

pub use self::_HD_RESULT as HD_RESULT;

pub type HD_DAL = crate::ctypes::c_uint;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_DEVCOUNT {
    pub max_dev_count: UINT32,
}

pub type HD_DEVCOUNT = _HD_DEVCOUNT;

pub const HD_VIDEOCAP_0_CTRL: _HD_CTRL_ID = 536936447;

pub const HD_VIDEOPROC_0_CTRL: _HD_CTRL_ID = 604045311;

pub const HD_AUDIOCAP_0_CTRL: _HD_CTRL_ID = 268500991;

pub const HD_AUDIOOUT_0_CTRL: _HD_CTRL_ID = 285278207;

pub type _HD_CTRL_ID = crate::ctypes::c_uint;

pub const HD_VIDEOCAP_0_IN_0: _HD_IN_ID = 536871168;

pub const HD_VIDEOENC_0_IN_0: _HD_IN_ID = 570425600;

pub const HD_VIDEOENC_0_IN_1: _HD_IN_ID = 570425856;

pub const HD_VIDEOENC_0_IN_2: _HD_IN_ID = 570426112;

pub const HD_VIDEOPROC_0_IN_0: _HD_IN_ID = 603980032;

pub const HD_AUDIOCAP_0_IN_0: _HD_IN_ID = 268435712;

pub const HD_AUDIOOUT_0_IN_0: _HD_IN_ID = 285212928;

pub const HD_AUDIOENC_0_IN_0: _HD_IN_ID = 301990144;

pub const HD_AUDIODEC_0_IN_10: _HD_IN_ID = 318769920;

pub const HD_AUDIODEC_0_IN_20: _HD_IN_ID = 318772480;

pub type _HD_IN_ID = crate::ctypes::c_uint;

pub use self::_HD_IN_ID as HD_IN_ID;

pub const HD_VIDEOCAP_0_OUT_0: _HD_OUT_ID = 536870913;

pub const HD_VIDEOENC_0_OUT_0: _HD_OUT_ID = 570425345;

pub const HD_VIDEOENC_0_OUT_1: _HD_OUT_ID = 570425346;

pub const HD_VIDEOENC_0_OUT_2: _HD_OUT_ID = 570425347;

pub const HD_VIDEOPROC_0_OUT_0: _HD_OUT_ID = 603979777;

pub const HD_VIDEOPROC_0_OUT_1: _HD_OUT_ID = 603979778;

pub const HD_VIDEOPROC_0_OUT_2: _HD_OUT_ID = 603979779;

pub const HD_AUDIOCAP_0_OUT_0: _HD_OUT_ID = 268435457;

pub const HD_AUDIOOUT_0_OUT_0: _HD_OUT_ID = 285212673;

pub const HD_AUDIOENC_0_OUT_0: _HD_OUT_ID = 301989889;

pub type _HD_OUT_ID = crate::ctypes::c_uint;

pub use self::_HD_OUT_ID as HD_OUT_ID;

pub type HD_PATH_ID = UINT32;

pub type _HD_DEVICE_CAPS = crate::ctypes::c_uint;

pub use self::_HD_DEVICE_CAPS as HD_DEVICE_CAPS;

pub type _HD_VIDEO_CAPS = crate::ctypes::c_uint;

pub use self::_HD_VIDEO_CAPS as HD_VIDEO_CAPS;

pub type _HD_AUDIO_CAPS = crate::ctypes::c_uint;

pub use self::_HD_AUDIO_CAPS as HD_AUDIO_CAPS;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_METADATA {
    pub sign: UINT32,
    pub p_next: *mut _HD_METADATA,
}

impl Default for _HD_METADATA {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_METADATA = _HD_METADATA;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_DIM {
    pub w: UINT32,
    pub h: UINT32,
}

pub type HD_DIM = _HD_DIM;

pub const HD_VIDEO_PXLFMT_RAW10: _HD_VIDEO_PXLFMT = 1091174400;

pub const HD_VIDEO_PXLFMT_YUV420: _HD_VIDEO_PXLFMT = 1376519200;

pub type _HD_VIDEO_PXLFMT = crate::ctypes::c_uint;

pub use self::_HD_VIDEO_PXLFMT as HD_VIDEO_PXLFMT;

pub type HD_VIDEO_DIR = crate::ctypes::c_uint;

pub type HD_VIDEO_FRC = crate::ctypes::c_uint;

pub type _HD_CROP_MODE = crate::ctypes::c_uint;

pub use self::_HD_CROP_MODE as HD_CROP_MODE;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_VIDEO_CROP {
    pub coord: HD_DIM,
    pub rect: HD_IRECT,
}

pub type HD_VIDEO_CROP = _HD_VIDEO_CROP;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_PALETTE_TBL {
    pub sign: UINT32,
    pub p_next: *mut HD_METADATA,
    pub reserved: [UINT32; 1usize],
    pub pxlfmt: HD_VIDEO_PXLFMT,
    pub table_size: UINT32,
    pub p_table: *mut UINT32,
}

impl Default for _HD_PALETTE_TBL {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEO_FRAME {
    pub sign: UINT32,
    pub p_next: *mut HD_METADATA,
    pub ddr_id: UINT32,
    pub pxlfmt: HD_VIDEO_PXLFMT,
    pub dim: HD_DIM,
    pub count: UINT64,
    pub timestamp: UINT64,
    pub pw: [UINT32; 4usize],
    pub ph: [UINT32; 4usize],
    pub loff: [UINT32; 4usize],
    pub phy_addr: [UINTPTR; 4usize],
    pub poc_info: UINT32,
    pub reserved: [UINT32; 7usize],
    pub blk: UINTPTR,
}

impl Default for _HD_VIDEO_FRAME {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_VIDEO_FRAME = _HD_VIDEO_FRAME;

pub const HD_CODEC_TYPE_JPEG: _HD_VIDEO_CODEC = 0;

pub const HD_CODEC_TYPE_H264: _HD_VIDEO_CODEC = 1;

pub type _HD_VIDEO_CODEC = crate::ctypes::c_uint;

pub use self::_HD_VIDEO_CODEC as HD_VIDEO_CODEC;

pub type _H264_NALU_TYPE = crate::ctypes::c_uint;

pub use self::_H264_NALU_TYPE as H264_NALU_TYPE;

pub type _H265_NALU_TYPE = crate::ctypes::c_uint;

pub use self::_H265_NALU_TYPE as H265_NALU_TYPE;

pub type _JPEG_PACK_TYPE = crate::ctypes::c_uint;

pub use self::_JPEG_PACK_TYPE as JPEG_PACK_TYPE;

#[repr(C)]
#[derive(Copy, Clone)]
pub union _HD_NALU_TYPE {
    pub h264_type: H264_NALU_TYPE,
    pub jpeg_type: JPEG_PACK_TYPE,
    pub h265_type: H265_NALU_TYPE,
}

impl Default for _HD_NALU_TYPE {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_NALU_TYPE = _HD_NALU_TYPE;

pub type _HD_SVC_LAYER_TYPE = crate::ctypes::c_uint;

pub use self::_HD_SVC_LAYER_TYPE as HD_SVC_LAYER_TYPE;

pub type _HD_VIDEO_EVBR_STATE = crate::ctypes::c_uint;

pub use self::_HD_VIDEO_EVBR_STATE as HD_VIDEO_EVBR_STATE;

pub const HD_FRAME_TYPE_IDR: _HD_FRAME_TYPE = 0;

pub const HD_FRAME_TYPE_I: _HD_FRAME_TYPE = 1;

pub type _HD_FRAME_TYPE = crate::ctypes::c_uint;

pub use self::_HD_FRAME_TYPE as HD_FRAME_TYPE;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_VIDEO_BS_PSNR_INFO {
    pub y_mse: UINT32,
    pub u_mse: UINT32,
    pub v_mse: UINT32,
}

pub type HD_VIDEO_BS_PSNR_INFO = _HD_VIDEO_BS_PSNR_INFO;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_VIDEO_H264_BLOCK_PARTITION_INFO {
    pub intra_16x16_mb_num: UINT32,
    pub intra_8x8_mb_Num: UINT32,
    pub intra_4x4_mb_num: UINT32,
    pub inter_mb_num: UINT32,
    pub skip_mb_num: UINT32,
}

pub type HD_VIDEO_H264_BLOCK_PARTITION_INFO = _HD_VIDEO_H264_BLOCK_PARTITION_INFO;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_VIDEO_H265_BLOCK_PARTITION_INFO {
    pub intra_32x32_cu_num: UINT32,
    pub intra_16x16_cu_num: UINT32,
    pub intra_8x8_cu_num: UINT32,
    pub inter_64x64_cu_num: UINT32,
    pub inter_32x32_cu_num: UINT32,
    pub inter_16x16_cu_num: UINT32,
    pub skip_cu_num: UINT32,
    pub merge_cu_num: UINT32,
}

pub type HD_VIDEO_H265_BLOCK_PARTITION_INFO = _HD_VIDEO_H265_BLOCK_PARTITION_INFO;

#[repr(C)]
#[derive(Copy, Clone)]
pub union _HD_VIDEO_BS_BLOCK_PARTITION_INFO {
    pub h264_info: HD_VIDEO_H264_BLOCK_PARTITION_INFO,
    pub h265_info: HD_VIDEO_H265_BLOCK_PARTITION_INFO,
}

impl Default for _HD_VIDEO_BS_BLOCK_PARTITION_INFO {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_VIDEO_BS_BLOCK_PARTITION_INFO = _HD_VIDEO_BS_BLOCK_PARTITION_INFO;

#[repr(C)]
#[derive(Copy, Clone)]
pub struct _HD_VIDEO_BS_DATA {
    pub pack_type: HD_NALU_TYPE,
    pub phy_addr: UINTPTR,
    pub size: UINT32,
}

impl Default for _HD_VIDEO_BS_DATA {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_VIDEO_BS_DATA = _HD_VIDEO_BS_DATA;

#[repr(C)]
#[derive(Copy, Clone)]
pub struct _HD_VIDEOENC_BS {
    pub sign: UINT32,
    pub p_next: *mut HD_METADATA,
    pub vcodec_format: HD_VIDEO_CODEC,
    pub pack_num: UINT32,
    pub timestamp: UINT64,
    pub frame_type: HD_FRAME_TYPE,
    pub svc_layer_type: HD_SVC_LAYER_TYPE,
    pub video_pack: [HD_VIDEO_BS_DATA; 4usize],
    pub psnr_info: HD_VIDEO_BS_PSNR_INFO,
    pub blk_info: HD_VIDEO_BS_BLOCK_PARTITION_INFO,
    pub evbr_state: HD_VIDEO_EVBR_STATE,
    pub qp: UINT32,
    pub ddr_id: UINT32,
    pub blk: UINTPTR,
    pub motion_ratio: UINT32,
}

impl Default for _HD_VIDEOENC_BS {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_VIDEOENC_BS = _HD_VIDEOENC_BS;

#[repr(C)]
#[derive(Copy, Clone)]
pub struct _HD_VIDEOENC_BS3 {
    pub sign: UINT32,
    pub p_next: *mut HD_METADATA,
    pub vcodec_format: HD_VIDEO_CODEC,
    pub pack_num: UINT32,
    pub timestamp: UINT64,
    pub frame_type: HD_FRAME_TYPE,
    pub svc_layer_type: HD_SVC_LAYER_TYPE,
    pub video_pack: *mut HD_VIDEO_BS_DATA,
    pub video_pack_num: UINT32,
    pub psnr_info: HD_VIDEO_BS_PSNR_INFO,
    pub blk_info: HD_VIDEO_BS_BLOCK_PARTITION_INFO,
    pub evbr_state: HD_VIDEO_EVBR_STATE,
    pub qp: UINT32,
    pub ddr_id: UINT32,
    pub blk: INT32,
    pub motion_ratio: UINT32,
}

impl Default for _HD_VIDEOENC_BS3 {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEODEC_BS {
    pub sign: UINT32,
    pub p_next: *mut HD_METADATA,
    pub vcodec_format: HD_VIDEO_CODEC,
    pub count: UINT64,
    pub timestamp: UINT64,
    pub ddr_id: UINT32,
    pub phy_addr: UINTPTR,
    pub size: UINT32,
    pub blk: INT32,
}

impl Default for _HD_VIDEODEC_BS {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type _HD_AUDIO_SR = crate::ctypes::c_uint;

pub use self::_HD_AUDIO_SR as HD_AUDIO_SR;

pub type _HD_AUDIO_BIT_WIDTH = crate::ctypes::c_uint;

pub use self::_HD_AUDIO_BIT_WIDTH as HD_AUDIO_BIT_WIDTH;

pub type _HD_AUDIO_SOUND_MODE = crate::ctypes::c_uint;

pub use self::_HD_AUDIO_SOUND_MODE as HD_AUDIO_SOUND_MODE;

pub type _HD_AUDIO_MONO = crate::ctypes::c_uint;

pub use self::_HD_AUDIO_MONO as HD_AUDIO_MONO;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_AUDIO_FRAME {
    pub sign: UINT32,
    pub p_next: *mut HD_METADATA,
    pub ddr_id: UINT32,
    pub size: UINT32,
    pub phy_addr: [UINTPTR; 2usize],
    pub bit_width: HD_AUDIO_BIT_WIDTH,
    pub sound_mode: HD_AUDIO_SOUND_MODE,
    pub sample_rate: HD_AUDIO_SR,
    pub reserved1: UINT32,
    pub count: UINT64,
    pub timestamp: UINT64,
    pub blk: INT32,
}

impl Default for _HD_AUDIO_FRAME {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_AUDIO_FRAME = _HD_AUDIO_FRAME;

pub const HD_AUDIO_CODEC_PCM: _HD_AUDIO_CODEC = 1;

pub const HD_AUDIO_CODEC_AAC: _HD_AUDIO_CODEC = 2;

pub type _HD_AUDIO_CODEC = crate::ctypes::c_uint;

pub use self::_HD_AUDIO_CODEC as HD_AUDIO_CODEC;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_AUDIO_CODEC_INFO {
    pub is_support: UINT32,
    pub support_sr: [HD_AUDIO_SR; 9usize],
    pub support_bit: [HD_AUDIO_BIT_WIDTH; 2usize],
    pub support_mode: [HD_AUDIO_SOUND_MODE; 3usize],
}

impl Default for _HD_AUDIO_CODEC_INFO {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_AUDIO_BS {
    pub sign: UINT32,
    pub p_next: *mut HD_METADATA,
    pub acodec_format: HD_AUDIO_CODEC,
    pub timestamp: UINT64,
    pub size: UINT32,
    pub ddr_id: UINT32,
    pub phy_addr: UINTPTR,
    pub blk: INT32,
}

impl Default for _HD_AUDIO_BS {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_AUDIO_BS = _HD_AUDIO_BS;

pub type _HD_OSG_MASK_TYPE = crate::ctypes::c_uint;

pub use self::_HD_OSG_MASK_TYPE as HD_OSG_MASK_TYPE;

pub type _HD_OSG_BUF_TYPE = crate::ctypes::c_uint;

pub use self::_HD_OSG_BUF_TYPE as HD_OSG_BUF_TYPE;

pub type _HD_OSG_ALIGN_TYPE = crate::ctypes::c_uint;

pub use self::_HD_OSG_ALIGN_TYPE as HD_OSG_ALIGN_TYPE;

pub type _HD_OSG_COPY_HW = crate::ctypes::c_uint;

pub use self::_HD_OSG_COPY_HW as HD_OSG_COPY_HW;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_OSG_STAMP_BUF {
    pub type_: HD_OSG_BUF_TYPE,
    pub size: UINT32,
    pub p_addr: UINTPTR,
    pub ddr_id: UINT8,
}

impl Default for _HD_OSG_STAMP_BUF {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_OSG_STAMP_IMG {
    pub fmt: HD_VIDEO_PXLFMT,
    pub dim: HD_DIM,
    pub p_addr: UINTPTR,
    pub ddr_id: UINT8,
    pub copy: HD_OSG_COPY_HW,
}

impl Default for _HD_OSG_STAMP_IMG {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_OSG_STAMP_ATTR {
    pub align_type: HD_OSG_ALIGN_TYPE,
    pub alpha: UINT32,
    pub position: HD_IPOINT,
    pub colorkey_en: UINT32,
    pub colorkey_val: UINT32,
    pub qp_en: UINT32,
    pub qp_fix: UINT32,
    pub qp_val: UINT32,
    pub layer: UINT32,
    pub region: UINT32,
    pub gcac_enable: UINT8,
    pub gcac_blk_width: UINT8,
    pub gcac_blk_height: UINT8,
}

impl Default for _HD_OSG_STAMP_ATTR {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_OSG_MASK_ATTR {
    pub type_: HD_OSG_MASK_TYPE,
    pub color: UINT32,
    pub alpha: UINT32,
    pub position: [HD_UPOINT; 4usize],
    pub thickness: UINT32,
}

impl Default for _HD_OSG_MASK_ATTR {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_OSG_MOSAIC_ATTR {
    pub type_: HD_OSG_MASK_TYPE,
    pub alpha: UINT32,
    pub mosaic_blk_w: UINT32,
    pub mosaic_blk_h: UINT32,
    pub position: [HD_UPOINT; 4usize],
}

impl Default for _HD_OSG_MOSAIC_ATTR {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

unsafe extern "C" {
    pub fn hd_gettime_ms() -> UINT32;
}

pub const HD_COMMON_VIDEO_IN_MIPI_CSI: _HD_COMMON_VIDEO_IN_TYPE = 5;

pub type _HD_COMMON_VIDEO_IN_TYPE = crate::ctypes::c_uint;

pub use self::_HD_COMMON_VIDEO_IN_TYPE as HD_COMMON_VIDEO_IN_TYPE;

pub const HD_COMMON_MEM_COMMON_POOL: _HD_COMMON_MEM_POOL_TYPE = 1;

pub type _HD_COMMON_MEM_POOL_TYPE = crate::ctypes::c_uint;

pub use self::_HD_COMMON_MEM_POOL_TYPE as HD_COMMON_MEM_POOL_TYPE;

pub const DDR_ID0: HD_COMMON_MEM_DDR_ID = 0;

pub type HD_COMMON_MEM_DDR_ID = crate::ctypes::c_uint;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_COMMON_MEM_POOL_INFO {
    pub ddr_id: HD_COMMON_MEM_DDR_ID,
    pub type_: HD_COMMON_MEM_POOL_TYPE,
    pub blk_size: UINT32,
    pub blk_cnt: UINT32,
    pub start_addr: UINTPTR,
    pub shared_pool: [HD_COMMON_MEM_POOL_TYPE; 1usize],
}

impl Default for _HD_COMMON_MEM_POOL_INFO {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_COMMON_MEM_POOL_INFO = _HD_COMMON_MEM_POOL_INFO;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_COMMON_MEM_CONFIG {
    pub pool_info: [HD_COMMON_MEM_POOL_INFO; 128usize],
}

impl Default for _HD_COMMON_MEM_CONFIG {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_COMMON_MEM_INIT_CONFIG = _HD_COMMON_MEM_CONFIG;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_COMMON_MEM_VIRT_INFO {
    pub va: *const crate::ctypes::c_void,
    pub pa: UINTPTR,
    pub cached: BOOL,
}

impl Default for _HD_COMMON_MEM_VIRT_INFO {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub const HD_COMMON_MEM_MEM_TYPE_CACHE: HD_COMMON_MEM_MEM_TYPE = 0;

pub type HD_COMMON_MEM_MEM_TYPE = crate::ctypes::c_uint;

unsafe extern "C" {
    pub fn hd_common_init(sys_config_type: UINT32) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_common_uninit() -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_common_mem_init(p_mem_config: *mut HD_COMMON_MEM_INIT_CONFIG) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_common_mem_mmap(
        mem_type: HD_COMMON_MEM_MEM_TYPE,
        phy_addr: UINTPTR,
        size: UINT32,
    ) -> *mut crate::ctypes::c_void;
}

unsafe extern "C" {
    pub fn hd_common_mem_munmap(
        virt_addr: *mut crate::ctypes::c_void,
        size: crate::ctypes::c_uint,
    ) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_common_mem_flush_cache(
        virt_addr: *mut crate::ctypes::c_void,
        size: crate::ctypes::c_uint,
    ) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_common_mem_alloc(
        name: *mut CHAR,
        phy_addr: *mut UINTPTR,
        virt_addr: *mut *mut crate::ctypes::c_void,
        size: UINT32,
        ddr: HD_COMMON_MEM_DDR_ID,
    ) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_common_mem_free(
        phy_addr: UINTPTR,
        virt_addr: *mut crate::ctypes::c_void,
    ) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_common_mem_uninit() -> HD_RESULT;
}

pub type _HD_VIDEOCAP_VI_SRC = crate::ctypes::c_uint;

pub use self::_HD_VIDEOCAP_VI_SRC as HD_VIDEOCAP_VI_SRC;

pub type _HD_VIDEOCAP_VI_FMT = crate::ctypes::c_uint;

pub use self::_HD_VIDEOCAP_VI_FMT as HD_VIDEOCAP_VI_FMT;

pub type _HD_VIDEOCAP_VI_TDM = crate::ctypes::c_uint;

pub use self::_HD_VIDEOCAP_VI_TDM as HD_VIDEOCAP_VI_TDM;

pub type _HD_VIDEOCAP_VI_CHID = crate::ctypes::c_uint;

pub use self::_HD_VIDEOCAP_VI_CHID as HD_VIDEOCAP_VI_CHID;

pub type _HD_VIDEOCAP_VI_LATCH_EDGE = crate::ctypes::c_uint;

pub use self::_HD_VIDEOCAP_VI_LATCH_EDGE as HD_VIDEOCAP_VI_LATCH_EDGE;

pub type _HD_VIDEOCAP_VI_VPORT_DATA_SWAP = crate::ctypes::c_uint;

pub use self::_HD_VIDEOCAP_VI_VPORT_DATA_SWAP as HD_VIDEOCAP_VI_VPORT_DATA_SWAP;

pub type _HD_VIDEOCAP_VI_CH_FMT = crate::ctypes::c_uint;

pub use self::_HD_VIDEOCAP_VI_CH_FMT as HD_VIDEOCAP_VI_CH_FMT;

pub type _HD_VIDEOCAP_VI_CH_PROG = crate::ctypes::c_uint;

pub use self::_HD_VIDEOCAP_VI_CH_PROG as HD_VIDEOCAP_VI_CH_PROG;

pub type _HD_VIDEOCAP_VI_CH_DATA_RATE = crate::ctypes::c_uint;

pub use self::_HD_VIDEOCAP_VI_CH_DATA_RATE as HD_VIDEOCAP_VI_CH_DATA_RATE;

pub type _HD_VIDEOCAP_VI_CH_DATA_LATCH = crate::ctypes::c_uint;

pub use self::_HD_VIDEOCAP_VI_CH_DATA_LATCH as HD_VIDEOCAP_VI_CH_DATA_LATCH;

pub type _HD_VIDEOCAP_VI_CH_HORIZ_DUP = crate::ctypes::c_uint;

pub use self::_HD_VIDEOCAP_VI_CH_HORIZ_DUP as HD_VIDEOCAP_VI_CH_HORIZ_DUP;

pub type _HD_VIDEOCAP_VI_CH_DATA_SRC = crate::ctypes::c_uint;

pub use self::_HD_VIDEOCAP_VI_CH_DATA_SRC as HD_VIDEOCAP_VI_CH_DATA_SRC;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOCAP_VI {
    pub chip: UINT8,
    pub vcap: UINT8,
    pub vi: UINT8,
    pub global: _HD_VIDEOCAP_VI__bindgen_ty_1,
    pub vport: [_HD_VIDEOCAP_VI__bindgen_ty_2; 2usize],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOCAP_VI__bindgen_ty_1 {
    pub src: HD_VIDEOCAP_VI_SRC,
    pub format: HD_VIDEOCAP_VI_FMT,
    pub tdm: HD_VIDEOCAP_VI_TDM,
    pub id_extract: HD_VIDEOCAP_VI_CHID,
    pub latch_edge: HD_VIDEOCAP_VI_LATCH_EDGE,
    pub reserved: [UINT32; 4usize],
}

impl Default for _HD_VIDEOCAP_VI__bindgen_ty_1 {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOCAP_VI__bindgen_ty_2 {
    pub data_swap: HD_VIDEOCAP_VI_VPORT_DATA_SWAP,
    pub clk_pin: UINT32,
    pub clk_inv: UINT32,
    pub clk_dly: UINT32,
    pub clk_pdly: UINT32,
    pub reserved: [UINT32; 4usize],
}

impl Default for _HD_VIDEOCAP_VI__bindgen_ty_2 {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

impl Default for _HD_VIDEOCAP_VI {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOCAP_VI_CH_NORM {
    pub chip: UINT8,
    pub vcap: UINT8,
    pub vi: UINT8,
    pub ch: UINT8,
    pub cap_width: UINT32,
    pub cap_height: UINT32,
    pub org_width: UINT32,
    pub org_height: UINT32,
    pub fps: UINT32,
    pub format: HD_VIDEOCAP_VI_CH_FMT,
    pub prog: HD_VIDEOCAP_VI_CH_PROG,
    pub data_rate: HD_VIDEOCAP_VI_CH_DATA_RATE,
    pub data_latch: HD_VIDEOCAP_VI_CH_DATA_LATCH,
    pub horiz_dup: HD_VIDEOCAP_VI_CH_HORIZ_DUP,
    pub reserved: [UINT32; 8usize],
}

impl Default for _HD_VIDEOCAP_VI_CH_NORM {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOCAP_VI_CH_NORM3 {
    pub chip: UINT8,
    pub vcap: UINT8,
    pub vi: UINT8,
    pub ch: UINT8,
    pub cap_width: UINT32,
    pub cap_height: UINT32,
    pub org_width: UINT32,
    pub org_height: UINT32,
    pub fps: UINT32,
    pub format: HD_VIDEOCAP_VI_CH_FMT,
    pub prog: HD_VIDEOCAP_VI_CH_PROG,
    pub data_rate: HD_VIDEOCAP_VI_CH_DATA_RATE,
    pub data_latch: HD_VIDEOCAP_VI_CH_DATA_LATCH,
    pub horiz_dup: HD_VIDEOCAP_VI_CH_HORIZ_DUP,
    pub data_src: HD_VIDEOCAP_VI_CH_DATA_SRC,
    pub reserved: [UINT32; 8usize],
}

impl Default for _HD_VIDEOCAP_VI_CH_NORM3 {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_VIDEOCAP_SEN_PINMUX_CFG {
    pub sensor_pinmux: UINT32,
    pub serial_if_pinmux: UINT32,
    pub cmd_if_pinmux: UINT32,
}

pub type HD_VIDEOCAP_SEN_PINMUX_CFG = _HD_VIDEOCAP_SEN_PINMUX_CFG;

pub const HD_VIDEOCAP_SEN_CLANE_SEL_CSI0_USE_C0: _HD_VIDEOCAP_SEN_CLANE_SEL = 0;

pub type _HD_VIDEOCAP_SEN_CLANE_SEL = crate::ctypes::c_uint;

pub use self::_HD_VIDEOCAP_SEN_CLANE_SEL as HD_VIDEOCAP_SEN_CLANE_SEL;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOCAP_SEN_INIT_PIN_CFG {
    pub pinmux: HD_VIDEOCAP_SEN_PINMUX_CFG,
    pub clk_lane_sel: HD_VIDEOCAP_SEN_CLANE_SEL,
    pub sen_2_serial_pin_map: [UINT32; 8usize],
    pub ccir_msblsb_switch: BOOL,
    pub ccir_vd_hd_pin: BOOL,
    pub vx1_tx241_cko_pin: BOOL,
    pub vx1_tx241_cfg_2lane_mode: BOOL,
}

impl Default for _HD_VIDEOCAP_SEN_INIT_PIN_CFG {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_VIDEOCAP_SEN_INIT_PIN_CFG = _HD_VIDEOCAP_SEN_INIT_PIN_CFG;

pub type _HD_VIDEOCAP_SEN_VX1_IF_SEL = crate::ctypes::c_uint;

pub use self::_HD_VIDEOCAP_SEN_VX1_IF_SEL as HD_VIDEOCAP_SEN_VX1_IF_SEL;

pub type _HD_VIDEOCAP_SEN_VX1_CTL_SEL = crate::ctypes::c_uint;

pub use self::_HD_VIDEOCAP_SEN_VX1_CTL_SEL as HD_VIDEOCAP_SEN_VX1_CTL_SEL;

pub type _HD_VIDEOCAP_SEN_VX1_TXTYPE = crate::ctypes::c_uint;

pub use self::_HD_VIDEOCAP_SEN_VX1_TXTYPE as HD_VIDEOCAP_SEN_VX1_TXTYPE;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOCAP_SEN_IF_VX1 {
    pub en: BOOL,
    pub if_sel: HD_VIDEOCAP_SEN_VX1_IF_SEL,
    pub ctl_sel: HD_VIDEOCAP_SEN_VX1_CTL_SEL,
    pub tx_type: HD_VIDEOCAP_SEN_VX1_TXTYPE,
}

impl Default for _HD_VIDEOCAP_SEN_IF_VX1 {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_VIDEOCAP_SEN_IF_VX1 = _HD_VIDEOCAP_SEN_IF_VX1;

pub type _HD_VIDEOCAP_SEN_TGE_CH_VD_TO_VCAP = crate::ctypes::c_uint;

pub use self::_HD_VIDEOCAP_SEN_TGE_CH_VD_TO_VCAP as HD_VIDEOCAP_SEN_TGE_CH_VD_TO_VCAP;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOCAP_SEN_IF_TGE {
    pub tge_en: BOOL,
    pub swap: BOOL,
    pub vcap_vd_src: HD_VIDEOCAP_SEN_TGE_CH_VD_TO_VCAP,
    pub vcap_sync_set: UINT32,
}

impl Default for _HD_VIDEOCAP_SEN_IF_TGE {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_VIDEOCAP_SEN_IF_TGE = _HD_VIDEOCAP_SEN_IF_TGE;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOCAP_SEN_INIT_IF_CFG {
    pub vx1: HD_VIDEOCAP_SEN_IF_VX1,
    pub tge: HD_VIDEOCAP_SEN_IF_TGE,
}

impl Default for _HD_VIDEOCAP_SEN_INIT_IF_CFG {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_VIDEOCAP_SEN_INIT_IF_CFG = _HD_VIDEOCAP_SEN_INIT_IF_CFG;

pub type _HD_VIDEOCAP_SEN_OPTION_ENABLE = crate::ctypes::c_uint;

pub use self::_HD_VIDEOCAP_SEN_OPTION_ENABLE as HD_VIDEOCAP_SEN_OPTION_ENABLE;

pub type _HD_VIDEOCAP_SEN_MAP_IF = crate::ctypes::c_uint;

pub use self::_HD_VIDEOCAP_SEN_MAP_IF as HD_VIDEOCAP_SEN_MAP_IF;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOCAP_SEN_INIT_OPTION {
    pub en_mask: HD_VIDEOCAP_SEN_OPTION_ENABLE,
    pub sen_map_if: HD_VIDEOCAP_SEN_MAP_IF,
    pub if_time_out: UINT32,
}

impl Default for _HD_VIDEOCAP_SEN_INIT_OPTION {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_VIDEOCAP_SEN_INIT_OPTION = _HD_VIDEOCAP_SEN_INIT_OPTION;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOCAP_SENSOR_DEVICE {
    pub driver_name: [CHAR; 32usize],
    pub if_type: HD_COMMON_VIDEO_IN_TYPE,
    pub if_cfg: HD_VIDEOCAP_SEN_INIT_IF_CFG,
    pub pin_cfg: HD_VIDEOCAP_SEN_INIT_PIN_CFG,
    pub option: HD_VIDEOCAP_SEN_INIT_OPTION,
}

impl Default for _HD_VIDEOCAP_SENSOR_DEVICE {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_VIDEOCAP_SENSOR_DEVICE = _HD_VIDEOCAP_SENSOR_DEVICE;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOCAP_SEN_CONFIG {
    pub sen_dev: HD_VIDEOCAP_SENSOR_DEVICE,
    pub shdr_map: UINT32,
}

impl Default for _HD_VIDEOCAP_SEN_CONFIG {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_VIDEOCAP_SEN_CONFIG = _HD_VIDEOCAP_SEN_CONFIG;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOCAP_CONFIG {
    pub sen_cfg: HD_VIDEOCAP_SEN_CONFIG,
}

impl Default for _HD_VIDEOCAP_CONFIG {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_VIDEOCAP_DRV_CONFIG = _HD_VIDEOCAP_CONFIG;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_VIDEOCAP_SEN_CFG_EXT {
    pub func: UINT32,
    pub value: UINT32,
}

pub type HD_VIDEOCAP_SEN_CFG_EXT = _HD_VIDEOCAP_SEN_CFG_EXT;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOCAP_SEN_CONFIG_EXT {
    pub cfg_num: UINT32,
    pub p_sen_cfg_ext: *mut HD_VIDEOCAP_SEN_CFG_EXT,
}

impl Default for _HD_VIDEOCAP_SEN_CONFIG_EXT {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_VIDEOCAP_SEN_CONFIG_EXT = _HD_VIDEOCAP_SEN_CONFIG_EXT;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOCAP_CONFIG3 {
    pub driver_name: [CHAR; 32usize],
    pub sen_cfg_ext: HD_VIDEOCAP_SEN_CONFIG_EXT,
}

impl Default for _HD_VIDEOCAP_CONFIG3 {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub const HD_VIDEOCAP_FUNC_AE: _HD_VIDEOCAP_CTRLFUNC = 256;

pub const HD_VIDEOCAP_FUNC_AWB: _HD_VIDEOCAP_CTRLFUNC = 512;

pub const HD_VIDEOCAP_FUNC_AF: _HD_VIDEOCAP_CTRLFUNC = 1024;

pub type _HD_VIDEOCAP_CTRLFUNC = crate::ctypes::c_uint;

pub use self::_HD_VIDEOCAP_CTRLFUNC as HD_VIDEOCAP_CTRLFUNC;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOCAP_CTRL {
    pub func: HD_VIDEOCAP_CTRLFUNC,
}

impl Default for _HD_VIDEOCAP_CTRL {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_VIDEOCAP_CTRL = _HD_VIDEOCAP_CTRL;

pub const HD_VIDEOCAP_SEN_FRAME_NUM_1: _HD_VIDEOCAP_SEN_FRAME_NUM = 1;

pub type _HD_VIDEOCAP_SEN_FRAME_NUM = crate::ctypes::c_uint;

pub use self::_HD_VIDEOCAP_SEN_FRAME_NUM as HD_VIDEOCAP_SEN_FRAME_NUM;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOCAP_IN {
    pub sen_mode: UINT32,
    pub frc: HD_VIDEO_FRC,
    pub dim: HD_DIM,
    pub pxlfmt: HD_VIDEO_PXLFMT,
    pub out_frame_num: HD_VIDEOCAP_SEN_FRAME_NUM,
}

impl Default for _HD_VIDEOCAP_IN {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_VIDEOCAP_IN = _HD_VIDEOCAP_IN;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOCAP_OUT {
    pub dim: HD_DIM,
    pub pxlfmt: HD_VIDEO_PXLFMT,
    pub dir: HD_VIDEO_DIR,
    pub frc: HD_VIDEO_FRC,
    pub depth: UINT32,
}

impl Default for _HD_VIDEOCAP_OUT {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_VIDEOCAP_OUT = _HD_VIDEOCAP_OUT;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_VIDEOCAP_CROP_AUTO_INFO {
    pub ratio_w_h: UINT32,
    pub factor: UINT32,
    pub sft: HD_IPOINT,
}

pub type HD_VIDEOCAP_CROP_AUTO_INFO = _HD_VIDEOCAP_CROP_AUTO_INFO;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOCAP_CROP {
    pub mode: HD_CROP_MODE,
    pub win: HD_VIDEO_CROP,
    pub auto_info: HD_VIDEOCAP_CROP_AUTO_INFO,
    pub align: HD_DIM,
}

impl Default for _HD_VIDEOCAP_CROP {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_VIDEOCAP_CROP = _HD_VIDEOCAP_CROP;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOCAP_SYSCAPS {
    pub dev_id: HD_DAL,
    pub chip_id: UINT32,
    pub max_in_count: UINT32,
    pub max_out_count: UINT32,
    pub dev_caps: HD_DEVICE_CAPS,
    pub in_caps: [HD_VIDEO_CAPS; 1usize],
    pub out_caps: [HD_VIDEO_CAPS; 8usize],
    pub max_dim: HD_DIM,
    pub max_frame_rate: UINT32,
    pub max_w_scaleup: UINT32,
    pub pxlfmt: HD_VIDEO_PXLFMT,
}

impl Default for _HD_VIDEOCAP_SYSCAPS {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_VIDEOCAP_SYSCAPS = _HD_VIDEOCAP_SYSCAPS;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_VIDEOCAP_SYSINFO {
    pub dev_id: HD_DAL,
    pub cur_fps: [UINT32; 8usize],
    pub vd_count: UINT64,
    pub output_started: BOOL,
    pub cur_dim: HD_DIM,
}

pub type HD_VIDEOCAP_SYSINFO = _HD_VIDEOCAP_SYSINFO;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_VIDEOCAP_MD_STATUS {
    pub enabled: BOOL,
}

pub type HD_VIDEOCAP_MD_STATUS = _HD_VIDEOCAP_MD_STATUS;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOCAP_POOL {
    pub ddr_id: INT,
    pub pxlfmt: HD_VIDEO_PXLFMT,
    pub dim: HD_DIM,
    pub counts: UINT32,
    pub max_counts: UINT32,
    pub min_counts: UINT32,
    pub mode: INT,
}

impl Default for _HD_VIDEOCAP_POOL {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_VIDEOCAP_POOL = _HD_VIDEOCAP_POOL;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOCAP_PATH_CONFIG {
    pub data_pool: [HD_VIDEOCAP_POOL; 4usize],
}

impl Default for _HD_VIDEOCAP_PATH_CONFIG {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type _HD_VIDEOCAP_OUTFUNC = crate::ctypes::c_uint;

pub use self::_HD_VIDEOCAP_OUTFUNC as HD_VIDEOCAP_OUTFUNC;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOCAP_FUNC_CONFIG {
    pub ddr_id: UINT32,
    pub out_func: HD_VIDEOCAP_OUTFUNC,
}

impl Default for _HD_VIDEOCAP_FUNC_CONFIG {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_VIDEOCAP_FUNC_CONFIG = _HD_VIDEOCAP_FUNC_CONFIG;

pub const HD_VIDEOCAP_PARAM_SYSCAPS: _HD_VIDEOCAP_PARAM_ID = 1;

pub const HD_VIDEOCAP_PARAM_SYSINFO: _HD_VIDEOCAP_PARAM_ID = 2;

pub const HD_VIDEOCAP_PARAM_DRV_CONFIG: _HD_VIDEOCAP_PARAM_ID = 3;

pub const HD_VIDEOCAP_PARAM_OUT: _HD_VIDEOCAP_PARAM_ID = 8;

pub const HD_VIDEOCAP_PARAM_CTRL: _HD_VIDEOCAP_PARAM_ID = 11;

pub const HD_VIDEOCAP_PARAM_IN: _HD_VIDEOCAP_PARAM_ID = 12;

pub type _HD_VIDEOCAP_PARAM_ID = crate::ctypes::c_uint;

pub use self::_HD_VIDEOCAP_PARAM_ID as HD_VIDEOCAP_PARAM_ID;

unsafe extern "C" {
    pub fn hd_videocap_init() -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_videocap_bind(out_id: HD_OUT_ID, dest_in_id: HD_IN_ID) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_videocap_unbind(out_id: HD_OUT_ID) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_videocap_open(
        in_id: HD_IN_ID,
        out_id: HD_OUT_ID,
        p_path_id: *mut HD_PATH_ID,
    ) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_videocap_start(path_id: HD_PATH_ID) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_videocap_stop(path_id: HD_PATH_ID) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_videocap_get(
        path_id: HD_PATH_ID,
        id: HD_VIDEOCAP_PARAM_ID,
        p_param: *mut VOID,
    ) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_videocap_set(
        path_id: HD_PATH_ID,
        id: HD_VIDEOCAP_PARAM_ID,
        p_param: *mut VOID,
    ) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_videocap_close(path_id: HD_PATH_ID) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_videocap_uninit() -> HD_RESULT;
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOPROC_SYSCAPS {
    pub dev_id: HD_DAL,
    pub chip_id: UINT32,
    pub max_in_count: UINT32,
    pub max_out_count: UINT32,
    pub dev_caps: HD_DEVICE_CAPS,
    pub in_caps: [HD_VIDEO_CAPS; 1usize],
    pub out_caps: [HD_VIDEO_CAPS; 16usize],
    pub max_w_scaleup_ratio: UINT32,
    pub max_w_scaledown_ratio: UINT32,
    pub max_h_scaleup_ratio: UINT32,
    pub max_h_scaledown_ratio: UINT32,
    pub max_in_stamp: UINT32,
    pub max_in_stamp_ex: UINT32,
    pub max_in_mask: UINT32,
    pub max_in_mask_ex: UINT32,
}

impl Default for _HD_VIDEOPROC_SYSCAPS {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_VIDEOPROC_SYSCAPS = _HD_VIDEOPROC_SYSCAPS;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_VIDEOPROC_SYSINFO {
    pub dev_id: HD_DAL,
    pub cur_in_fps: [UINT32; 1usize],
    pub cur_out_fps: [UINT32; 16usize],
}

pub type HD_VIDEOPROC_SYSINFO = _HD_VIDEOPROC_SYSINFO;

pub type _HD_VIDEOPROC_INFUNC = crate::ctypes::c_uint;

pub use self::_HD_VIDEOPROC_INFUNC as HD_VIDEOPROC_INFUNC;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOPROC_CROP {
    pub mode: HD_CROP_MODE,
    pub win: HD_VIDEO_CROP,
}

impl Default for _HD_VIDEOPROC_CROP {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_VIDEOPROC_CROP = _HD_VIDEOPROC_CROP;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_VIDEOPROC_FRC {
    pub frc: HD_VIDEO_FRC,
}

pub type HD_VIDEOPROC_FRC = _HD_VIDEOPROC_FRC;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOPROC_IN {
    pub func: UINT32,
    pub dim: HD_DIM,
    pub pxlfmt: HD_VIDEO_PXLFMT,
    pub dir: HD_VIDEO_DIR,
    pub frc: HD_VIDEO_FRC,
}

impl Default for _HD_VIDEOPROC_IN {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_VIDEOPROC_IN = _HD_VIDEOPROC_IN;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOPROC_IN3 {
    pub func: UINT32,
    pub dim: HD_DIM,
    pub pxlfmt: HD_VIDEO_PXLFMT,
    pub dir: HD_VIDEO_DIR,
    pub frc: HD_VIDEO_FRC,
    pub depth: UINT32,
}

impl Default for _HD_VIDEOPROC_IN3 {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type _HD_VIDEOPROC_OUTFUNC = crate::ctypes::c_uint;

pub use self::_HD_VIDEOPROC_OUTFUNC as HD_VIDEOPROC_OUTFUNC;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOPROC_OUT {
    pub func: UINT32,
    pub dim: HD_DIM,
    pub pxlfmt: HD_VIDEO_PXLFMT,
    pub dir: HD_VIDEO_DIR,
    pub frc: HD_VIDEO_FRC,
    pub depth: UINT32,
    pub rect: HD_URECT,
    pub bg: HD_DIM,
}

impl Default for _HD_VIDEOPROC_OUT {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_VIDEOPROC_OUT = _HD_VIDEOPROC_OUT;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOPROC_OUT_EX {
    pub src_path: HD_PATH_ID,
    pub dim: HD_DIM,
    pub pxlfmt: HD_VIDEO_PXLFMT,
    pub dir: HD_VIDEO_DIR,
    pub frc: HD_VIDEO_FRC,
    pub depth: UINT32,
}

impl Default for _HD_VIDEOPROC_OUT_EX {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_VIDEOPROC_OUT_EX = _HD_VIDEOPROC_OUT_EX;

pub type _HD_VIDEOPROC_PALETTE = crate::ctypes::c_uint;

pub use self::_HD_VIDEOPROC_PALETTE as HD_VIDEOPROC_PALETTE;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOPROC_PATTERN_IMG {
    pub index: UINT32,
    pub image: HD_VIDEO_FRAME,
}

impl Default for _HD_VIDEOPROC_PATTERN_IMG {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOPROC_PATTERN_SELECT {
    pub index: UINT32,
    pub rect: HD_URECT,
    pub bg_color_sel: HD_VIDEOPROC_PALETTE,
    pub img_crop_enable: UINT32,
    pub img_crop: HD_URECT,
}

impl Default for _HD_VIDEOPROC_PATTERN_SELECT {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub const HD_VIDEOPROC_FUNC_AF: _HD_VIDEOPROC_CTRLFUNC = 33554432;

pub type _HD_VIDEOPROC_CTRLFUNC = crate::ctypes::c_uint;

pub use self::_HD_VIDEOPROC_CTRLFUNC as HD_VIDEOPROC_CTRLFUNC;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOPROC_CTRL {
    pub func: HD_VIDEOPROC_CTRLFUNC,
    pub ref_path_3dnr: HD_PATH_ID,
}

impl Default for _HD_VIDEOPROC_CTRL {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_VIDEOPROC_CTRL = _HD_VIDEOPROC_CTRL;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_VIDEOPROC_LL_CONFIG {
    pub delay_trig_lowlatency: UINT32,
}

pub type HD_VIDEOPROC_LL_CONFIG = _HD_VIDEOPROC_LL_CONFIG;

pub const HD_VIDEOPROC_PIPE_RAWALL: _HD_VIDEOPROC_PIPE = 254;

pub type _HD_VIDEOPROC_PIPE = crate::ctypes::c_uint;

pub use self::_HD_VIDEOPROC_PIPE as HD_VIDEOPROC_PIPE;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_VIDEOPROC_POOL {
    pub ddr_id: INT,
    pub counts: UINT32,
    pub max_counts: UINT32,
    pub min_counts: UINT32,
    pub mode: INT,
}

pub type HD_VIDEOPROC_POOL = _HD_VIDEOPROC_POOL;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOPROC_DEV_CONFIG {
    pub pipe: HD_VIDEOPROC_PIPE,
    pub isp_id: UINT32,
    pub ctrl_max: HD_VIDEOPROC_CTRL,
    pub in_max: HD_VIDEOPROC_IN,
    pub data_pool: [HD_VIDEOPROC_POOL; 4usize],
}

impl Default for _HD_VIDEOPROC_DEV_CONFIG {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_VIDEOPROC_DEV_CONFIG = _HD_VIDEOPROC_DEV_CONFIG;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOPROC_FUNC_CONFIG {
    pub ddr_id: UINT32,
    pub in_func: HD_VIDEOPROC_INFUNC,
    pub out_func: HD_VIDEOPROC_OUTFUNC,
    pub out_order: UINT32,
}

impl Default for _HD_VIDEOPROC_FUNC_CONFIG {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type _HD_VIDEOPROC_ALIGN = crate::ctypes::c_uint;

pub use self::_HD_VIDEOPROC_ALIGN as HD_VIDEOPROC_ALIGN;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOPROC_USER_BS {
    pub sign: UINT32,
    pub p_next: *mut HD_METADATA,
    pub p_bs_buf: *mut CHAR,
    pub bs_buf_size: UINT32,
    pub retval: INT32,
    pub time_align: HD_VIDEOPROC_ALIGN,
    pub time_diff: UINT32,
    pub timestamp: UINT64,
    pub user_flag: UINT32,
}

impl Default for _HD_VIDEOPROC_USER_BS {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_VIDEOPROC_USER_BS = _HD_VIDEOPROC_USER_BS;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOPROC_SEND_LIST {
    pub path_id: HD_PATH_ID,
    pub user_bs: HD_VIDEOPROC_USER_BS,
    pub retval: INT32,
}

impl Default for _HD_VIDEOPROC_SEND_LIST {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub const HD_VIDEOPROC_PARAM_DEV_CONFIG: _HD_VIDEOPROC_PARAM_ID = 3;

pub const HD_VIDEOPROC_PARAM_CTRL: _HD_VIDEOPROC_PARAM_ID = 4;

pub const HD_VIDEOPROC_PARAM_OUT: _HD_VIDEOPROC_PARAM_ID = 9;

pub type _HD_VIDEOPROC_PARAM_ID = crate::ctypes::c_uint;

pub use self::_HD_VIDEOPROC_PARAM_ID as HD_VIDEOPROC_PARAM_ID;

unsafe extern "C" {
    pub fn hd_videoproc_init() -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_videoproc_bind(out_id: HD_OUT_ID, dest_in_id: HD_IN_ID) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_videoproc_unbind(out_id: HD_OUT_ID) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_videoproc_open(
        in_id: HD_IN_ID,
        out_id: HD_OUT_ID,
        p_path_id: *mut HD_PATH_ID,
    ) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_videoproc_start(path_id: HD_PATH_ID) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_videoproc_stop(path_id: HD_PATH_ID) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_videoproc_set(
        path_id: HD_PATH_ID,
        id: HD_VIDEOPROC_PARAM_ID,
        p_param: *mut VOID,
    ) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_videoproc_close(path_id: HD_PATH_ID) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_videoproc_uninit() -> HD_RESULT;
}

pub type _HD_VIDEOENC_CAPS = crate::ctypes::c_uint;

pub use self::_HD_VIDEOENC_CAPS as HD_VIDEOENC_CAPS;

pub const HD_SVC_DISABLE: _HD_VIDEOENC_SVC_LAYER = 0;

pub type _HD_VIDEOENC_SVC_LAYER = crate::ctypes::c_uint;

pub use self::_HD_VIDEOENC_SVC_LAYER as HD_VIDEOENC_SVC_LAYER;

pub const HD_H264E_MAIN_PROFILE: _HD_VIDEOENC_PROFILE = 77;

pub type _HD_VIDEOENC_PROFILE = crate::ctypes::c_uint;

pub use self::_HD_VIDEOENC_PROFILE as HD_VIDEOENC_PROFILE;

pub const HD_H264E_LEVEL_4_1: _HD_VIDEOENC_LEVEL = 41;

pub type _HD_VIDEOENC_LEVEL = crate::ctypes::c_uint;

pub use self::_HD_VIDEOENC_LEVEL as HD_VIDEOENC_LEVEL;

pub const HD_H264E_CABAC_CODING: _HD_VIDEOENC_CODING = 1;

pub type _HD_VIDEOENC_CODING = crate::ctypes::c_uint;

pub use self::_HD_VIDEOENC_CODING as HD_VIDEOENC_CODING;

pub const HD_RC_MODE_CBR: _HD_VIDEOENC_RC_MODE = 1;

pub type _HD_VIDEOENC_RC_MODE = crate::ctypes::c_uint;

pub use self::_HD_VIDEOENC_RC_MODE as HD_VIDEOENC_RC_MODE;

pub type _HD_VIDEOENC_QPMODE = crate::ctypes::c_uint;

pub use self::_HD_VIDEOENC_QPMODE as HD_VIDEOENC_QPMODE;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOENC_SYSCAPS {
    pub dev_id: HD_DAL,
    pub chip_id: UINT32,
    pub max_in_count: UINT32,
    pub max_out_count: UINT32,
    pub dev_caps: HD_DEVICE_CAPS,
    pub in_caps: [HD_VIDEO_CAPS; 96usize],
    pub out_caps: [HD_VIDEOENC_CAPS; 96usize],
    pub max_in_stamp: UINT32,
    pub max_in_stamp_ex: UINT32,
    pub max_in_mask: UINT32,
    pub max_in_mask_ex: UINT32,
}

impl Default for _HD_VIDEOENC_SYSCAPS {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_VIDEOENC_BUFINFO {
    pub buf_info: HD_BUFINFO,
}

pub type HD_VIDEOENC_BUFINFO = _HD_VIDEOENC_BUFINFO;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOENC_MAXMEM {
    pub codec_type: HD_VIDEO_CODEC,
    pub max_dim: HD_DIM,
    pub bitrate: UINT32,
    pub enc_buf_ms: UINT32,
    pub svc_layer: HD_VIDEOENC_SVC_LAYER,
    pub ltr: BOOL,
    pub rotate: BOOL,
    pub source_output: BOOL,
}

impl Default for _HD_VIDEOENC_MAXMEM {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_VIDEOENC_MAXMEM = _HD_VIDEOENC_MAXMEM;

pub type _HD_VIDEOENC_POOL_MODE = crate::ctypes::c_uint;

pub use self::_HD_VIDEOENC_POOL_MODE as HD_VIDEOENC_POOL_MODE;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOENC_POOL {
    pub ddr_id: INT,
    pub counts: UINT32,
    pub max_counts: UINT32,
    pub min_counts: UINT32,
    pub mode: HD_VIDEOENC_POOL_MODE,
}

impl Default for _HD_VIDEOENC_POOL {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_VIDEOENC_POOL = _HD_VIDEOENC_POOL;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOENC_PATH_CONFIG {
    pub max_mem: HD_VIDEOENC_MAXMEM,
    pub isp_id: UINT32,
    pub data_pool: [HD_VIDEOENC_POOL; 4usize],
}

impl Default for _HD_VIDEOENC_PATH_CONFIG {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_VIDEOENC_PATH_CONFIG = _HD_VIDEOENC_PATH_CONFIG;

pub type _HD_VIDEOENC_INFUNC = crate::ctypes::c_uint;

pub use self::_HD_VIDEOENC_INFUNC as HD_VIDEOENC_INFUNC;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOENC_FUNC_CONFIG {
    pub ddr_id: UINT32,
    pub in_func: HD_VIDEOENC_INFUNC,
}

impl Default for _HD_VIDEOENC_FUNC_CONFIG {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_VIDEOENC_FUNC_CONFIG = _HD_VIDEOENC_FUNC_CONFIG;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_VIDEOENC_FRC {
    pub frc: HD_VIDEO_FRC,
}

pub type HD_VIDEOENC_FRC = _HD_VIDEOENC_FRC;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOENC_IN {
    pub dim: HD_DIM,
    pub pxl_fmt: HD_VIDEO_PXLFMT,
    pub dir: HD_VIDEO_DIR,
    pub frc: HD_VIDEO_FRC,
}

impl Default for _HD_VIDEOENC_IN {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_VIDEOENC_IN = _HD_VIDEOENC_IN;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_H26X_CONFIG {
    pub gop_num: UINT32,
    pub chrm_qp_idx: INT8,
    pub sec_chrm_qp_idx: INT8,
    pub ltr_interval: UINT32,
    pub ltr_pre_ref: BOOL,
    pub gray_en: BOOL,
    pub source_output: BOOL,
    pub profile: HD_VIDEOENC_PROFILE,
    pub level_idc: HD_VIDEOENC_LEVEL,
    pub svc_layer: HD_VIDEOENC_SVC_LAYER,
    pub entropy_mode: HD_VIDEOENC_CODING,
}

impl Default for _HD_H26X_CONFIG {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_H26X_CONFIG = _HD_H26X_CONFIG;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_JPEG_CONFIG {
    pub retstart_interval: UINT32,
    pub image_quality: UINT32,
}

pub type HD_JPEG_CONFIG = _HD_JPEG_CONFIG;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_JPEG_CONFIG2 {
    pub retstart_interval: UINT32,
    pub image_quality: UINT32,
    pub bitrate: UINT32,
    pub frame_rate_base: UINT32,
    pub frame_rate_incr: UINT32,
}

pub type HD_JPEG_CONFIG2 = _HD_JPEG_CONFIG2;

#[repr(C)]
#[derive(Copy, Clone)]
pub struct _HD_VIDEOENC_OUT {
    pub codec_type: HD_VIDEO_CODEC,
    pub __bindgen_anon_1: _HD_VIDEOENC_OUT__bindgen_ty_1,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub union _HD_VIDEOENC_OUT__bindgen_ty_1 {
    pub h26x: HD_H26X_CONFIG,
    pub jpeg: HD_JPEG_CONFIG,
}

impl Default for _HD_VIDEOENC_OUT__bindgen_ty_1 {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

impl Default for _HD_VIDEOENC_OUT {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_VIDEOENC_OUT = _HD_VIDEOENC_OUT;

#[repr(C)]
#[derive(Copy, Clone)]
pub struct _HD_VIDEOENC_OUT2 {
    pub codec_type: HD_VIDEO_CODEC,
    pub __bindgen_anon_1: _HD_VIDEOENC_OUT2__bindgen_ty_1,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub union _HD_VIDEOENC_OUT2__bindgen_ty_1 {
    pub h26x: HD_H26X_CONFIG,
    pub jpeg: HD_JPEG_CONFIG2,
}

impl Default for _HD_VIDEOENC_OUT2__bindgen_ty_1 {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

impl Default for _HD_VIDEOENC_OUT2 {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_VIDEOENC_OUT2 = _HD_VIDEOENC_OUT2;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_H26XENC_VUI {
    pub vui_en: BOOL,
    pub sar_width: UINT32,
    pub sar_height: UINT32,
    pub matrix_coef: UINT8,
    pub transfer_characteristics: UINT8,
    pub colour_primaries: UINT8,
    pub video_format: UINT8,
    pub color_range: UINT8,
    pub timing_present_flag: BOOL,
}

pub type HD_H26XENC_VUI = _HD_H26XENC_VUI;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_H26XENC_DEBLOCK {
    pub dis_ilf_idc: UINT8,
    pub db_alpha: INT8,
    pub db_beta: INT8,
}

pub type HD_H26XENC_DEBLOCK = _HD_H26XENC_DEBLOCK;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_H26XENC_CBR {
    pub bitrate: UINT32,
    pub frame_rate_base: UINT32,
    pub frame_rate_incr: UINT32,
    pub init_i_qp: UINT8,
    pub max_i_qp: UINT8,
    pub min_i_qp: UINT8,
    pub init_p_qp: UINT8,
    pub max_p_qp: UINT8,
    pub min_p_qp: UINT8,
    pub static_time: UINT32,
    pub ip_weight: INT32,
}

pub type HD_H26XENC_CBR = _HD_H26XENC_CBR;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_H26XENC_CBR2 {
    pub bitrate: UINT32,
    pub frame_rate_base: UINT32,
    pub frame_rate_incr: UINT32,
    pub init_i_qp: UINT8,
    pub max_i_qp: UINT8,
    pub min_i_qp: UINT8,
    pub init_p_qp: UINT8,
    pub max_p_qp: UINT8,
    pub min_p_qp: UINT8,
    pub static_time: UINT32,
    pub ip_weight: INT32,
    pub key_p_period: UINT32,
    pub kp_weight: INT32,
    pub p2_weight: INT32,
    pub p3_weight: INT32,
    pub lt_weight: INT32,
    pub motion_aq_str: INT32,
    pub max_frame_size: UINT32,
}

pub type HD_H26XENC_CBR2 = _HD_H26XENC_CBR2;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_H26XENC_VBR {
    pub bitrate: UINT32,
    pub frame_rate_base: UINT32,
    pub frame_rate_incr: UINT32,
    pub init_i_qp: UINT8,
    pub max_i_qp: UINT8,
    pub min_i_qp: UINT8,
    pub init_p_qp: UINT8,
    pub max_p_qp: UINT8,
    pub min_p_qp: UINT8,
    pub static_time: UINT32,
    pub ip_weight: INT32,
    pub change_pos: UINT32,
}

pub type HD_H26XENC_VBR = _HD_H26XENC_VBR;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_H26XENC_VBR2 {
    pub bitrate: UINT32,
    pub frame_rate_base: UINT32,
    pub frame_rate_incr: UINT32,
    pub init_i_qp: UINT8,
    pub max_i_qp: UINT8,
    pub min_i_qp: UINT8,
    pub init_p_qp: UINT8,
    pub max_p_qp: UINT8,
    pub min_p_qp: UINT8,
    pub static_time: UINT32,
    pub ip_weight: INT32,
    pub change_pos: UINT32,
    pub key_p_period: UINT32,
    pub kp_weight: INT32,
    pub p2_weight: INT32,
    pub p3_weight: INT32,
    pub lt_weight: INT32,
    pub motion_aq_str: INT32,
    pub max_frame_size: UINT32,
}

pub type HD_H26XENC_VBR2 = _HD_H26XENC_VBR2;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_H26XENC_FIXQP {
    pub frame_rate_base: UINT32,
    pub frame_rate_incr: UINT32,
    pub fix_i_qp: UINT8,
    pub fix_p_qp: UINT8,
}

pub type HD_H26XENC_FIXQP = _HD_H26XENC_FIXQP;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_H26XENC_EVBR {
    pub bitrate: UINT32,
    pub frame_rate_base: UINT32,
    pub frame_rate_incr: UINT32,
    pub init_i_qp: UINT8,
    pub max_i_qp: UINT8,
    pub min_i_qp: UINT8,
    pub init_p_qp: UINT8,
    pub max_p_qp: UINT8,
    pub min_p_qp: UINT8,
    pub static_time: UINT32,
    pub ip_weight: INT32,
    pub key_p_period: UINT32,
    pub kp_weight: INT32,
    pub still_frame_cnd: UINT32,
    pub motion_ratio_thd: UINT32,
    pub motion_aq_str: INT32,
    pub still_i_qp: UINT32,
    pub still_p_qp: UINT32,
    pub still_kp_qp: UINT32,
}

pub type HD_H26XENC_EVBR = _HD_H26XENC_EVBR;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_H26XENC_EVBR2 {
    pub bitrate: UINT32,
    pub frame_rate_base: UINT32,
    pub frame_rate_incr: UINT32,
    pub init_i_qp: UINT8,
    pub max_i_qp: UINT8,
    pub min_i_qp: UINT8,
    pub init_p_qp: UINT8,
    pub max_p_qp: UINT8,
    pub min_p_qp: UINT8,
    pub static_time: UINT32,
    pub ip_weight: INT32,
    pub key_p_period: UINT32,
    pub kp_weight: INT32,
    pub still_frame_cnd: UINT32,
    pub motion_ratio_thd: UINT32,
    pub motion_aq_str: INT32,
    pub still_i_qp: UINT32,
    pub still_p_qp: UINT32,
    pub still_kp_qp: UINT32,
    pub p2_weight: INT32,
    pub p3_weight: INT32,
    pub lt_weight: INT32,
    pub max_frame_size: UINT32,
}

pub type HD_H26XENC_EVBR2 = _HD_H26XENC_EVBR2;

#[repr(C)]
#[derive(Copy, Clone)]
pub struct _HD_H26XENC_RATE_CONTROL {
    pub rc_mode: HD_VIDEOENC_RC_MODE,
    pub __bindgen_anon_1: _HD_H26XENC_RATE_CONTROL__bindgen_ty_1,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub union _HD_H26XENC_RATE_CONTROL__bindgen_ty_1 {
    pub cbr: HD_H26XENC_CBR,
    pub vbr: HD_H26XENC_VBR,
    pub fixqp: HD_H26XENC_FIXQP,
    pub evbr: HD_H26XENC_EVBR,
}

impl Default for _HD_H26XENC_RATE_CONTROL__bindgen_ty_1 {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

impl Default for _HD_H26XENC_RATE_CONTROL {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_H26XENC_RATE_CONTROL = _HD_H26XENC_RATE_CONTROL;

#[repr(C)]
#[derive(Copy, Clone)]
pub struct _HD_H26XENC_RATE_CONTROL2 {
    pub rc_mode: HD_VIDEOENC_RC_MODE,
    pub __bindgen_anon_1: _HD_H26XENC_RATE_CONTROL2__bindgen_ty_1,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub union _HD_H26XENC_RATE_CONTROL2__bindgen_ty_1 {
    pub cbr: HD_H26XENC_CBR2,
    pub vbr: HD_H26XENC_VBR2,
    pub fixqp: HD_H26XENC_FIXQP,
    pub evbr: HD_H26XENC_EVBR2,
}

impl Default for _HD_H26XENC_RATE_CONTROL2__bindgen_ty_1 {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

impl Default for _HD_H26XENC_RATE_CONTROL2 {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_H26XENC_RATE_CONTROL2 = _HD_H26XENC_RATE_CONTROL2;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_H26XENC_USR_QP {
    pub enable: BOOL,
    pub qp_map_addr: UINTPTR,
}

pub type HD_H26XENC_USR_QP = _HD_H26XENC_USR_QP;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_H26XENC_SLICE_SPLIT {
    pub enable: UINT32,
    pub slice_row_num: UINT32,
}

pub type HD_H26XENC_SLICE_SPLIT = _HD_H26XENC_SLICE_SPLIT;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_H26XENC_GDR {
    pub enable: BOOL,
    pub period: UINT32,
    pub number: UINT32,
}

pub type HD_H26XENC_GDR = _HD_H26XENC_GDR;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_H26XENC_ROI_WIN {
    pub enable: BOOL,
    pub rect: HD_URECT,
    pub mode: HD_VIDEOENC_QPMODE,
    pub qp: INT8,
}

impl Default for _HD_H26XENC_ROI_WIN {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_H26XENC_ROI_WIN = _HD_H26XENC_ROI_WIN;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_H26XENC_ROI {
    pub roi_qp_mode: HD_VIDEOENC_QPMODE,
    pub st_roi: [HD_H26XENC_ROI_WIN; 10usize],
}

impl Default for _HD_H26XENC_ROI {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_H26XENC_ROI = _HD_H26XENC_ROI;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_H26XENC_ROW_RC {
    pub enable: BOOL,
    pub i_qp_range: UINT8,
    pub i_qp_step: UINT8,
    pub p_qp_range: UINT8,
    pub p_qp_step: UINT8,
    pub min_i_qp: UINT8,
    pub max_i_qp: UINT8,
    pub min_p_qp: UINT8,
    pub max_p_qp: UINT8,
}

pub type HD_H26XENC_ROW_RC = _HD_H26XENC_ROW_RC;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_H26XENC_AQ {
    pub enable: BOOL,
    pub i_str: UINT8,
    pub p_str: UINT8,
    pub max_delta_qp: INT8,
    pub min_delta_qp: INT8,
    pub depth: UINT8,
    pub thd_table: [INT16; 30usize],
}

pub type HD_H26XENC_AQ = _HD_H26XENC_AQ;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_VIDEO_ENC_QP_RATIO {
    pub qp: UINT8,
    pub ratio: UINT8,
}

pub type HD_VIDEO_ENC_QP_RATIO = _HD_VIDEO_ENC_QP_RATIO;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEO_ENC_MIN_COMPRESS_RATIO {
    pub codec_type: HD_VIDEO_CODEC,
    pub qp_ratio: HD_VIDEO_ENC_QP_RATIO,
}

impl Default for _HD_VIDEO_ENC_MIN_COMPRESS_RATIO {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_H26XENC_REQUEST_IFRAME {
    pub enable: BOOL,
}

pub type HD_H26XENC_REQUEST_IFRAME = _HD_H26XENC_REQUEST_IFRAME;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_H26XENC_TRIG_SNAPSHOT {
    pub phy_addr: UINTPTR,
    pub size: UINT32,
    pub image_quality: UINT32,
}

pub type HD_H26XENC_TRIG_SNAPSHOT = _HD_H26XENC_TRIG_SNAPSHOT;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct HD_ENC_RET_EVENT {
    pub event: BOOL,
    pub bs_size: UINT32,
    pub frame_type: HD_FRAME_TYPE,
    pub svc_layer_type: HD_SVC_LAYER_TYPE,
}

impl Default for HD_ENC_RET_EVENT {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_VIDEOENC_POLL_LIST {
    pub path_id: HD_PATH_ID,
    pub revent: HD_ENC_RET_EVENT,
}

impl Default for _HD_VIDEOENC_POLL_LIST {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_VIDEOENC_POLL_LIST = _HD_VIDEOENC_POLL_LIST;

#[repr(C)]
#[derive(Copy, Clone)]
pub struct _HD_VIDEOENC_USER_BS_DATA {
    pub pack_type: HD_NALU_TYPE,
    pub user_buf_addr: UINTPTR,
    pub size: UINT32,
}

impl Default for _HD_VIDEOENC_USER_BS_DATA {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_VIDEOENC_USER_BS_DATA = _HD_VIDEOENC_USER_BS_DATA;

#[repr(C)]
#[derive(Copy, Clone)]
pub struct _HD_VIDEOENC_USER_BS {
    pub sign: UINT32,
    pub p_next: *mut HD_METADATA,
    pub vcodec_format: HD_VIDEO_CODEC,
    pub pack_num: UINT32,
    pub timestamp: UINT64,
    pub frame_type: HD_FRAME_TYPE,
    pub svc_layer_type: HD_SVC_LAYER_TYPE,
    pub video_pack: [HD_VIDEOENC_USER_BS_DATA; 4usize],
    pub psnr_info: HD_VIDEO_BS_PSNR_INFO,
    pub blk_info: HD_VIDEO_BS_BLOCK_PARTITION_INFO,
    pub newbs_flag: UINT32,
    pub qp: UINT32,
    pub evbr_state: HD_VIDEO_EVBR_STATE,
    pub slice_offset: [UINT16; 16usize],
    pub motion_ratio: UINT32,
    pub p_user_buf: *mut CHAR,
    pub user_buf_size: UINT32,
}

impl Default for _HD_VIDEOENC_USER_BS {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_VIDEOENC_USER_BS = _HD_VIDEOENC_USER_BS;

#[repr(C)]
#[derive(Copy, Clone)]
pub struct _HD_VIDEOENC_RECV_LIST {
    pub path_id: HD_PATH_ID,
    pub user_bs: HD_VIDEOENC_USER_BS,
    pub retval: INT32,
}

impl Default for _HD_VIDEOENC_RECV_LIST {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub const HD_VIDEOENC_PARAM_PATH_CONFIG: _HD_VIDEOENC_PARAM_ID = 2;

pub const HD_VIDEOENC_PARAM_BUFINFO: _HD_VIDEOENC_PARAM_ID = 3;

pub const HD_VIDEOENC_PARAM_IN: _HD_VIDEOENC_PARAM_ID = 4;

pub const HD_VIDEOENC_PARAM_OUT_ENC_PARAM: _HD_VIDEOENC_PARAM_ID = 5;

pub const HD_VIDEOENC_PARAM_OUT_RATE_CONTROL: _HD_VIDEOENC_PARAM_ID = 8;

pub type _HD_VIDEOENC_PARAM_ID = crate::ctypes::c_uint;

pub use self::_HD_VIDEOENC_PARAM_ID as HD_VIDEOENC_PARAM_ID;

unsafe extern "C" {
    pub fn hd_videoenc_init() -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_videoenc_open(
        in_id: HD_IN_ID,
        out_id: HD_OUT_ID,
        p_path_id: *mut HD_PATH_ID,
    ) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_videoenc_start(path_id: HD_PATH_ID) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_videoenc_stop(path_id: HD_PATH_ID) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_videoenc_close(path_id: HD_PATH_ID) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_videoenc_get(
        path_id: HD_PATH_ID,
        id: HD_VIDEOENC_PARAM_ID,
        p_param: *mut VOID,
    ) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_videoenc_set(
        path_id: HD_PATH_ID,
        id: HD_VIDEOENC_PARAM_ID,
        p_param: *mut VOID,
    ) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_videoenc_pull_out_buf(
        path_id: HD_PATH_ID,
        p_videoenc_bs: *mut HD_VIDEOENC_BS,
        wait_ms: INT32,
    ) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_videoenc_release_out_buf(
        path_id: HD_PATH_ID,
        p_videoenc_bs: *mut HD_VIDEOENC_BS,
    ) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_videoenc_poll_list(
        p_poll: *mut HD_VIDEOENC_POLL_LIST,
        num: UINT32,
        wait_ms: INT32,
    ) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_videoenc_uninit() -> HD_RESULT;
}

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_AUDIOCAP_SSP_CONFIG {
    pub ssp_num: [INT; 4usize],
    pub enable: [INT; 4usize],
    pub ssp_chan: [INT; 4usize],
    pub sample_size: [INT; 4usize],
    pub sample_rate: [INT; 4usize],
    pub ssp_clock: [INT; 4usize],
    pub bit_clock: [INT; 4usize],
    pub ssp_master: [INT; 4usize],
    pub live_sound_ch: [INT; 4usize],
}

pub type HD_AUDIOCAP_SSP_CONFIG = _HD_AUDIOCAP_SSP_CONFIG;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_AUDIOCAP_DRV_CONFIG {
    pub mono: HD_AUDIO_MONO,
    pub ssp_config: HD_AUDIOCAP_SSP_CONFIG,
}

impl Default for _HD_AUDIOCAP_DRV_CONFIG {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_AUDIOCAP_DRV_CONFIG = _HD_AUDIOCAP_DRV_CONFIG;

pub type _HD_AUDIOCAP_SRCAPS = crate::ctypes::c_uint;

pub use self::_HD_AUDIOCAP_SRCAPS as HD_AUDIOCAP_SRCAPS;

pub const HD_AUDIOCAP_LB_CH_LEFT: _HD_AUDIOCAP_LB_CH = 0;

pub type _HD_AUDIOCAP_LB_CH = crate::ctypes::c_uint;

pub use self::_HD_AUDIOCAP_LB_CH as HD_AUDIOCAP_LB_CH;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_AUDIOCAP_SYSINFO {
    pub dev_id: HD_DAL,
    pub cur_in_sample_rate: HD_AUDIO_SR,
    pub cur_sample_bit: HD_AUDIO_BIT_WIDTH,
    pub cur_mode: HD_AUDIO_SOUND_MODE,
    pub cur_out_sample_rate: [HD_AUDIO_SR; 16usize],
}

impl Default for _HD_AUDIOCAP_SYSINFO {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_AUDIOCAP_SYSINFO = _HD_AUDIOCAP_SYSINFO;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_AUDIOCAP_IN {
    pub sample_rate: HD_AUDIO_SR,
    pub sample_bit: HD_AUDIO_BIT_WIDTH,
    pub mode: HD_AUDIO_SOUND_MODE,
    pub frame_sample: UINT32,
}

impl Default for _HD_AUDIOCAP_IN {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_AUDIOCAP_IN = _HD_AUDIOCAP_IN;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_AUDIOCAP_OUT {
    pub sample_rate: HD_AUDIO_SR,
}

impl Default for _HD_AUDIOCAP_OUT {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_AUDIOCAP_OUT = _HD_AUDIOCAP_OUT;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_AUDIOCAP_AEC {
    pub enabled: BOOL,
    pub leak_estimate_enabled: BOOL,
    pub leak_estimate_value: INT32,
    pub noise_cancel_level: INT32,
    pub echo_cancel_level: INT32,
    pub filter_length: INT32,
    pub frame_size: INT32,
    pub notch_radius: INT32,
    pub lb_channel: HD_AUDIOCAP_LB_CH,
}

impl Default for _HD_AUDIOCAP_AEC {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_AUDIOCAP_AEC = _HD_AUDIOCAP_AEC;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_AUDIOCAP_ANR {
    pub enabled: BOOL,
    pub suppress_level: INT32,
    pub hpf_cut_off_freq: INT32,
    pub bias_sensitive: INT32,
}

pub type HD_AUDIOCAP_ANR = _HD_AUDIOCAP_ANR;

pub type _HD_AUDIOCAP_POOL_MODE = crate::ctypes::c_uint;

pub use self::_HD_AUDIOCAP_POOL_MODE as HD_AUDIOCAP_POOL_MODE;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_AUDIOCAP_POOL {
    pub ddr_id: INT,
    pub frame_sample_size: UINT32,
    pub counts: UINT32,
    pub max_counts: UINT32,
    pub min_counts: UINT32,
    pub mode: HD_AUDIOCAP_POOL_MODE,
}

impl Default for _HD_AUDIOCAP_POOL {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_AUDIOCAP_POOL = _HD_AUDIOCAP_POOL;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_AUDIOCAP_DEV_CONFIG {
    pub in_max: HD_AUDIOCAP_IN,
    pub frame_num_max: UINT32,
    pub out_max: HD_AUDIOCAP_OUT,
    pub aec_max: HD_AUDIOCAP_AEC,
    pub anr_max: HD_AUDIOCAP_ANR,
    pub data_pool: [HD_AUDIOCAP_POOL; 4usize],
}

impl Default for _HD_AUDIOCAP_DEV_CONFIG {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_AUDIOCAP_DEV_CONFIG = _HD_AUDIOCAP_DEV_CONFIG;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_AUDIOCAP_SYSCAPS {
    pub dev_id: HD_DAL,
    pub chip_id: UINT32,
    pub max_in_count: UINT32,
    pub max_out_count: UINT32,
    pub dev_caps: HD_DEVICE_CAPS,
    pub in_caps: [HD_AUDIO_CAPS; 1usize],
    pub out_caps: [HD_AUDIO_CAPS; 16usize],
    pub support_in_sr: [HD_AUDIOCAP_SRCAPS; 1usize],
    pub support_out_sr: [HD_AUDIOCAP_SRCAPS; 16usize],
}

impl Default for _HD_AUDIOCAP_SYSCAPS {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_AUDIOCAP_SYSCAPS = _HD_AUDIOCAP_SYSCAPS;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_AUDIOCAP_VOLUME {
    pub volume: UINT32,
}

pub type HD_AUDIOCAP_VOLUME = _HD_AUDIOCAP_VOLUME;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_AUDIOCAP_BUFINFO {
    pub buf_info: HD_BUFINFO,
}

pub type HD_AUDIOCAP_BUFINFO = _HD_AUDIOCAP_BUFINFO;

pub const HD_AUDIOCAP_PARAM_DEV_CONFIG: _HD_AUDIOCAP_PARAM_ID = 3;

pub const HD_AUDIOCAP_PARAM_DRV_CONFIG: _HD_AUDIOCAP_PARAM_ID = 4;

pub const HD_AUDIOCAP_PARAM_IN: _HD_AUDIOCAP_PARAM_ID = 5;

pub const HD_AUDIOCAP_PARAM_OUT: _HD_AUDIOCAP_PARAM_ID = 6;

pub const HD_AUDIOCAP_PARAM_OUT_AEC: _HD_AUDIOCAP_PARAM_ID = 7;

pub const HD_AUDIOCAP_PARAM_OUT_ANR: _HD_AUDIOCAP_PARAM_ID = 8;

pub const HD_AUDIOCAP_PARAM_VOLUME: _HD_AUDIOCAP_PARAM_ID = 9;

pub const HD_AUDIOCAP_PARAM_BUFINFO: _HD_AUDIOCAP_PARAM_ID = 10;

pub type _HD_AUDIOCAP_PARAM_ID = crate::ctypes::c_uint;

pub use self::_HD_AUDIOCAP_PARAM_ID as HD_AUDIOCAP_PARAM_ID;

unsafe extern "C" {
    pub fn hd_audiocap_init() -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_audiocap_bind(out_id: HD_OUT_ID, dest_in_id: HD_IN_ID) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_audiocap_unbind(out_id: HD_OUT_ID) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_audiocap_open(
        in_id: HD_IN_ID,
        out_id: HD_OUT_ID,
        p_path_id: *mut HD_PATH_ID,
    ) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_audiocap_start(path_id: HD_PATH_ID) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_audiocap_stop(path_id: HD_PATH_ID) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_audiocap_set(
        path_id: HD_PATH_ID,
        id: HD_AUDIOCAP_PARAM_ID,
        p_param: *mut VOID,
    ) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_audiocap_get(
        path_id: HD_PATH_ID,
        id: HD_AUDIOCAP_PARAM_ID,
        p_param: *mut VOID,
    ) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_audiocap_pull_out_buf(
        path_id: HD_PATH_ID,
        p_audio_frame: *mut HD_AUDIO_FRAME,
        wait_ms: INT32,
    ) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_audiocap_release_out_buf(
        path_id: HD_PATH_ID,
        p_audio_frame: *mut HD_AUDIO_FRAME,
    ) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_audiocap_close(path_id: HD_PATH_ID) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_audiocap_uninit() -> HD_RESULT;
}

pub type _HD_AUDIOENC_CAPS = crate::ctypes::c_uint;

pub use self::_HD_AUDIOENC_CAPS as HD_AUDIOENC_CAPS;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_AUDIOENC_SYSCAPS {
    pub dev_id: HD_DAL,
    pub chip_id: UINT32,
    pub max_in_count: UINT32,
    pub max_out_count: UINT32,
    pub dev_caps: HD_DEVICE_CAPS,
    pub in_caps: [HD_AUDIO_CAPS; 64usize],
    pub out_caps: [HD_AUDIOENC_CAPS; 64usize],
}

impl Default for _HD_AUDIOENC_SYSCAPS {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_AUDIOENC_BUFINFO {
    pub buf_info: HD_BUFINFO,
}

pub type HD_AUDIOENC_BUFINFO = _HD_AUDIOENC_BUFINFO;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_AUDIOENC_MAXMEM {
    pub codec_type: HD_AUDIO_CODEC,
    pub sample_rate: HD_AUDIO_SR,
    pub sample_bit: HD_AUDIO_BIT_WIDTH,
    pub mode: HD_AUDIO_SOUND_MODE,
}

impl Default for _HD_AUDIOENC_MAXMEM {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_AUDIOENC_MAXMEM = _HD_AUDIOENC_MAXMEM;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_AUDIOENC_PATH_CONFIG {
    pub max_mem: HD_AUDIOENC_MAXMEM,
}

impl Default for _HD_AUDIOENC_PATH_CONFIG {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_AUDIOENC_PATH_CONFIG = _HD_AUDIOENC_PATH_CONFIG;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_AUDIOENC_MAXMEM2 {
    pub codec_type: HD_AUDIO_CODEC,
    pub sample_rate: HD_AUDIO_SR,
    pub sample_bit: HD_AUDIO_BIT_WIDTH,
    pub mode: HD_AUDIO_SOUND_MODE,
    pub buf_ms: UINT32,
}

impl Default for _HD_AUDIOENC_MAXMEM2 {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_AUDIOENC_MAXMEM2 = _HD_AUDIOENC_MAXMEM2;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_AUDIOENC_PATH_CONFIG2 {
    pub max_mem: HD_AUDIOENC_MAXMEM2,
}

impl Default for _HD_AUDIOENC_PATH_CONFIG2 {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_AUDIOENC_PATH_CONFIG2 = _HD_AUDIOENC_PATH_CONFIG2;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_AUDIOENC_IN {
    pub sample_rate: HD_AUDIO_SR,
    pub sample_bit: HD_AUDIO_BIT_WIDTH,
    pub mode: HD_AUDIO_SOUND_MODE,
}

impl Default for _HD_AUDIOENC_IN {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_AUDIOENC_IN = _HD_AUDIOENC_IN;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_AUDIOENC_OUT {
    pub codec_type: HD_AUDIO_CODEC,
    pub aac_adts: UINT32,
}

impl Default for _HD_AUDIOENC_OUT {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_AUDIOENC_OUT = _HD_AUDIOENC_OUT;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_AUDIOENC_USER_BS {
    pub sign: UINT32,
    pub p_next: *mut HD_METADATA,
    pub acodec_format: HD_AUDIO_CODEC,
    pub timestamp: UINT64,
    pub size: UINT32,
    pub newbs_flag: UINT32,
    pub p_user_buf: *mut CHAR,
    pub user_buf_size: UINT32,
}

impl Default for _HD_AUDIOENC_USER_BS {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_AUDIOENC_USER_BS = _HD_AUDIOENC_USER_BS;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_AUDIOENC_RECV_LIST {
    pub path_id: HD_PATH_ID,
    pub user_bs: HD_AUDIOENC_USER_BS,
    pub retval: INT32,
}

impl Default for _HD_AUDIOENC_RECV_LIST {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub const HD_AUDIOENC_PARAM_BUFINFO: _HD_AUDIOENC_PARAM_ID = 2;

pub const HD_AUDIOENC_PARAM_PATH_CONFIG: _HD_AUDIOENC_PARAM_ID = 3;

pub const HD_AUDIOENC_PARAM_IN: _HD_AUDIOENC_PARAM_ID = 4;

pub const HD_AUDIOENC_PARAM_OUT: _HD_AUDIOENC_PARAM_ID = 5;

pub type _HD_AUDIOENC_PARAM_ID = crate::ctypes::c_uint;

pub use self::_HD_AUDIOENC_PARAM_ID as HD_AUDIOENC_PARAM_ID;

unsafe extern "C" {
    pub fn hd_audioenc_init() -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_audioenc_open(
        in_id: HD_IN_ID,
        out_id: HD_OUT_ID,
        p_path_id: *mut HD_PATH_ID,
    ) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_audioenc_start(path_id: HD_PATH_ID) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_audioenc_stop(path_id: HD_PATH_ID) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_audioenc_get(
        path_id: HD_PATH_ID,
        id: HD_AUDIOENC_PARAM_ID,
        p_param: *mut VOID,
    ) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_audioenc_set(
        path_id: HD_PATH_ID,
        id: HD_AUDIOENC_PARAM_ID,
        p_param: *mut VOID,
    ) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_audioenc_pull_out_buf(
        path_id: HD_PATH_ID,
        p_audio_bs: *mut HD_AUDIO_BS,
        wait_ms: INT32,
    ) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_audioenc_release_out_buf(
        path_id: HD_PATH_ID,
        p_audio_bs: *mut HD_AUDIO_BS,
    ) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_audioenc_close(path_id: HD_PATH_ID) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_audioenc_uninit() -> HD_RESULT;
}

pub type _HD_AUDIOOUT_OUTPUT = crate::ctypes::c_uint;

pub use self::_HD_AUDIOOUT_OUTPUT as HD_AUDIOOUT_OUTPUT;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_AUDIOOUT_SSP_CONFIG {
    pub enable: [INT; 4usize],
    pub resample_ratio: [INT; 4usize],
    pub playback_chmap: [INT; 4usize],
}

pub type HD_AUDIOOUT_SSP_CONFIG = _HD_AUDIOOUT_SSP_CONFIG;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_AUDIOOUT_DRV_CONFIG {
    pub mono: HD_AUDIO_MONO,
    pub output: HD_AUDIOOUT_OUTPUT,
    pub ssp_config: HD_AUDIOOUT_SSP_CONFIG,
}

impl Default for _HD_AUDIOOUT_DRV_CONFIG {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_AUDIOOUT_DRV_CONFIG = _HD_AUDIOOUT_DRV_CONFIG;

pub type _HD_AUDIOOUT_SRCAPS = crate::ctypes::c_uint;

pub use self::_HD_AUDIOOUT_SRCAPS as HD_AUDIOOUT_SRCAPS;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_AUDIOOUT_SYSINFO {
    pub dev_id: HD_DAL,
    pub cur_out_sample_rate: HD_AUDIO_SR,
    pub cur_sample_bit: HD_AUDIO_BIT_WIDTH,
    pub cur_mode: HD_AUDIO_SOUND_MODE,
    pub cur_in_sample_rate: [HD_AUDIO_SR; 2usize],
}

impl Default for _HD_AUDIOOUT_SYSINFO {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_AUDIOOUT_SYSINFO = _HD_AUDIOOUT_SYSINFO;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_AUDIOOUT_OUT {
    pub sample_rate: HD_AUDIO_SR,
    pub sample_bit: HD_AUDIO_BIT_WIDTH,
    pub mode: HD_AUDIO_SOUND_MODE,
}

impl Default for _HD_AUDIOOUT_OUT {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_AUDIOOUT_OUT = _HD_AUDIOOUT_OUT;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_AUDIOOUT_IN {
    pub sample_rate: HD_AUDIO_SR,
}

impl Default for _HD_AUDIOOUT_IN {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_AUDIOOUT_IN = _HD_AUDIOOUT_IN;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_AUDIOOUT_DEV_CONFIG {
    pub out_max: HD_AUDIOOUT_OUT,
    pub frame_sample_max: UINT32,
    pub frame_num_max: UINT32,
    pub in_max: HD_AUDIOOUT_IN,
}

impl Default for _HD_AUDIOOUT_DEV_CONFIG {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_AUDIOOUT_DEV_CONFIG = _HD_AUDIOOUT_DEV_CONFIG;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _HD_AUDIOOUT_SYSCAPS {
    pub dev_id: HD_DAL,
    pub chip_id: UINT32,
    pub max_in_count: UINT32,
    pub max_out_count: UINT32,
    pub dev_caps: HD_DEVICE_CAPS,
    pub in_caps: [HD_AUDIO_CAPS; 2usize],
    pub out_caps: [HD_AUDIO_CAPS; 1usize],
    pub support_in_sr: [HD_AUDIOOUT_SRCAPS; 2usize],
    pub support_out_sr: [HD_AUDIOOUT_SRCAPS; 1usize],
}

impl Default for _HD_AUDIOOUT_SYSCAPS {
    fn default() -> Self {
        let mut s = ::core::mem::MaybeUninit::<Self>::uninit();
        unsafe {
            ::core::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

pub type HD_AUDIOOUT_SYSCAPS = _HD_AUDIOOUT_SYSCAPS;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct _HD_AUDIOOUT_VOLUME {
    pub volume: UINT32,
}

pub type HD_AUDIOOUT_VOLUME = _HD_AUDIOOUT_VOLUME;

pub const HD_AUDIOOUT_PARAM_DEV_CONFIG: _HD_AUDIOOUT_PARAM_ID = 3;

pub const HD_AUDIOOUT_PARAM_DRV_CONFIG: _HD_AUDIOOUT_PARAM_ID = 4;

pub const HD_AUDIOOUT_PARAM_OUT: _HD_AUDIOOUT_PARAM_ID = 5;

pub const HD_AUDIOOUT_PARAM_IN: _HD_AUDIOOUT_PARAM_ID = 6;

pub const HD_AUDIOOUT_PARAM_VOLUME: _HD_AUDIOOUT_PARAM_ID = 7;

pub type _HD_AUDIOOUT_PARAM_ID = crate::ctypes::c_uint;

pub use self::_HD_AUDIOOUT_PARAM_ID as HD_AUDIOOUT_PARAM_ID;

unsafe extern "C" {
    pub fn hd_audioout_init() -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_audioout_open(
        in_id: HD_IN_ID,
        out_id: HD_OUT_ID,
        p_path_id: *mut HD_PATH_ID,
    ) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_audioout_start(path_id: HD_PATH_ID) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_audioout_stop(path_id: HD_PATH_ID) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_audioout_set(
        path_id: HD_PATH_ID,
        id: HD_AUDIOOUT_PARAM_ID,
        p_param: *mut VOID,
    ) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_audioout_push_in_buf(
        path_id: HD_PATH_ID,
        p_in_audio_frame: *mut HD_AUDIO_FRAME,
        wait_ms: INT32,
    ) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_audioout_close(path_id: HD_PATH_ID) -> HD_RESULT;
}

unsafe extern "C" {
    pub fn hd_audioout_uninit() -> HD_RESULT;
}
