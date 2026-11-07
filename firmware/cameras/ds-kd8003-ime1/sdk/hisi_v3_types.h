/* HiSilicon Hi3516CV300 (MPP V1.0.x "v3") structure declarations used by the media bridge.
 * Adapted from OpenIPC Divinus hal/hisi v3 headers (MIT License, https://github.com/OpenIPC/divinus).
 * Declarations only; layouts were checked against the shipped SDK (profile 1008 B, VENC pack 136 B).
 *
 * Copyright (c) 2024 OpenIPC
 *
 * Permission is hereby granted, free of charge, to any person obtaining a copy of this software and
 * associated documentation files (the "Software"), to deal in the Software without restriction,
 * including without limitation the rights to use, copy, modify, merge, publish, distribute,
 * sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in all copies or
 * substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT
 * NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
 * NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES
 * OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
 * CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE. */
#ifndef HISI_V3_TYPES_H
#define HISI_V3_TYPES_H

/* v3_common.h */



typedef enum {
    V3_BAYER_RG,
    V3_BAYER_GR,
    V3_BAYER_GB,
    V3_BAYER_BG,
    V3_BAYER_END
} v3_common_bayer;

typedef enum {
    V3_COMPR_NONE,
    V3_COMPR_SEG,
    V3_COMPR_SEG128,
    V3_COMPR_LINE,
    V3_COMPR_FRAME,
    V3_COMPR_END
} v3_common_compr;

typedef enum {
    V3_PIXFMT_1BPP,
    V3_PIXFMT_2BPP,
    V3_PIXFMT_4BPP,
    V3_PIXFMT_8BPP,
    V3_PIXFMT_RGB444,
    V3_PIXFMT_ARGB4444,
    V3_PIXFMT_RGB555,
    V3_PIXFMT_RGB565,
    V3_PIXFMT_ARGB1555,
    V3_PIXFMT_RGB888,
    V3_PIXFMT_ARGB8888,
    V3_PIXFMT_RGB888P,
    V3_PIXFMT_RGB_BAYER_8BPP,
    V3_PIXFMT_RGB_BAYER_10BPP,
    V3_PIXFMT_RGB_BAYER_12BPP,
    V3_PIXFMT_RGB_BAYER_14BPP,
    V3_PIXFMT_RGB_BAYER_16BPP,
    V3_PIXFMT_YUV_A422,
    V3_PIXFMT_YUV_A444,
    V3_PIXFMT_YUV422P,
    V3_PIXFMT_YUV420P,
    V3_PIXFMT_YUV444P,
    V3_PIXFMT_YUV422SP,
    V3_PIXFMT_YUV420SP,
    V3_PIXFMT_YUV444SP,
    V3_PIXFMT_YUV422_UYVY,
    V3_PIXFMT_YUV422_YUYV,
    V3_PIXFMT_YUV422_VYUY,
    V3_PIXFMT_YCbCrP,
    V3_PIXFMT_YUV400,
    V3_PIXFMT_END
} v3_common_pixfmt;

typedef enum {
    V3_PREC_8BPP,
    V3_PREC_10BPP,
    V3_PREC_12BPP,
    V3_PREC_14BPP,
    V3_PREC_16BPP,
    V3_PREC_END
} v3_common_prec;

typedef enum {
    V3_VIDFMT_LINEAR,
    V3_VIDFMT_TILE_256X16,
    V3_VIDFMT_TILE_64X16,
    V3_VIDFMT_END
} v3_common_vidfmt;

typedef enum {
    V3_WDR_NONE,
    V3_WDR_BUILTIN,
    V3_WDR_QUDRA,
    V3_WDR_2TO1_LINE,
    V3_WDR_2TO1_FRAME,
    V3_WDR_2TO1_FRAME_FULLRATE,
    V3_WDR_3TO1_LINE,
    V3_WDR_3TO1_FRAME,
    V3_WDR_3TO1_FRAME_FULLRATE,
    V3_WDR_4TO1_LINE,
    V3_WDR_4TO1_FRAME,
    V3_WDR_4TO1_FRAME_FULLRATE,
    V3_WDR_END
} v3_common_wdr;

typedef struct {
    unsigned int topWidth;
    unsigned int bottomWidth;
    unsigned int leftWidth;
    unsigned int rightWidth;
    unsigned int color;
} v3_common_bord;

typedef struct {
    unsigned int width;
    unsigned int height;
} v3_common_dim;

typedef struct {
    int x;
    int y;
} v3_common_pnt;

typedef struct {
    int x;
    int y;
    unsigned int width;
    unsigned int height;
} v3_common_rect;

/* v3_sys.h */


#define V3_SYS_API "1.0"

#define V3_SYS_TEMP_PREP 0x60fa0000
#define V3A_SYS_TEMP_CFG 0x120a0110
#define V3A_SYS_TEMP_VAL 0x120a0118
#define V3_SYS_TEMP_CFG  0x1203009c
#define V3_SYS_TEMP_VAL  0x120300a4

typedef enum {
    V3_SYS_MOD_CMPI,
    V3_SYS_MOD_VB,
    V3_SYS_MOD_SYS,
    V3_SYS_MOD_RGN,
    V3_SYS_MOD_CHNL,
    V3_SYS_MOD_VDEC,
    V3_SYS_MOD_GROUP,
    V3_SYS_MOD_VPSS,
    V3_SYS_MOD_VENC,
    V3_SYS_MOD_VDA,
    V3_SYS_MOD_H264E,
    V3_SYS_MOD_JPEGE,
    V3_SYS_MOD_MPEG4E,
    V3_SYS_MOD_H264D,
    V3_SYS_MOD_JPEGD,
    V3_SYS_MOD_VOU,
    V3_SYS_MOD_VIU,
    V3_SYS_MOD_DSU,
    V3_SYS_MOD_VALG,
    V3_SYS_MOD_RC,
    V3_SYS_MOD_AIO,
    V3_SYS_MOD_AI,
    V3_SYS_MOD_AO,
    V3_SYS_MOD_AENC,
    V3_SYS_MOD_ADEC,
    V3_SYS_MOD_AVENC,
    V3_SYS_MOD_PCIV,
    V3_SYS_MOD_PCIVFMW,
    V3_SYS_MOD_ISP,
    V3_SYS_MOD_IVE,
    V3_SYS_MOD_DCCM = 31,
    V3_SYS_MOD_DCCS,
    V3_SYS_MOD_PROC,
    V3_SYS_MOD_LOG,
    V3_SYS_MOD_MST_LOG,
    V3_SYS_MOD_VD,
    V3_SYS_MOD_VCMP = 38,
    V3_SYS_MOD_FB,
    V3_SYS_MOD_HDMI,
    V3_SYS_MOD_VOIE,
    V3_SYS_MOD_TDE,
    V3_SYS_MOD_USR,
    V3_SYS_MOD_VEDU,
    V3_SYS_MOD_VGS,
    V3_SYS_MOD_H265E,
    V3_SYS_MOD_FD,
    V3_SYS_MOD_ODT,
    V3_SYS_MOD_VQA,
    V3_SYS_MOD_LPR,
    V3_SYS_MOD_FISHEYE,
    V3_SYS_MOD_PHOTO,
    V3_SYS_MOD_EXTAO,
    V3_SYS_MOD_END
} v3_sys_mod;

typedef struct {
    v3_sys_mod module;
    int device;
    int channel;
} v3_sys_bind;

typedef struct {
    char version[64];
} v3_sys_ver;

typedef struct {
    void *handle, *handleVoiceEngine, *handleDnvqe, *handleUpvqe;
    
    int (*fnExit)(void);
    int (*fnGetChipId)(unsigned int *chip);
    int (*fnGetVersion)(v3_sys_ver *version);
    
    int (*fnInit)(void);
    int (*fnSetAlignment)(unsigned int *width);

    int (*fnBind)(v3_sys_bind *source, v3_sys_bind *dest);
    int (*fnUnbind)(v3_sys_bind *source, v3_sys_bind *dest);

    int (*fnGetViVpssMode)(unsigned int *onlineOn);
} v3_sys_impl;


/* v3_snr.h */



#define V3_SNR_IOC_MAGIC 'm'
#define V3_SNR_LVDS_LANE_NUM 4
#define V3_SNR_MIPI_LANE_NUM 4
#define V3_SNR_WDR_VC_NUM 4

enum {
    V3_SNR_CMD_CONF_DEV = 1,
    V3_SNR_CMD_CONF_EDGE,
    V3_SNR_CMD_CONF_MSB,
    V3_SNR_CMD_CONF_CMV,
    V3_SNR_CMD_RST_SENS,
    V3_SNR_CMD_UNRST_SENS,
    V3_SNR_CMD_RST_MIPI,
    V3_SNR_CMD_UNRST_MIPI,
    V3_SNR_CMD_CONF_CROP
};

typedef enum {
    V3_SNR_INPUT_MIPI,
    V3_SNR_INPUT_SUBLVDS,
    V3_SNR_INPUT_LVDS,
    V3_SNR_INPUT_HISPI,
    V3_SNR_INPUT_CMOS_18V,
    V3_SNR_INPUT_CMOS_33V,
    V3_SNR_INPUT_BT1120,
    V3_SNR_INPUT_END
} v3_snr_input;

typedef enum {
    V3_SNR_LFID_NONE,
    V3_SNR_LFID_INSAV,
    V3_SNR_LFID_INDATA,
    V3_SNR_LFID_END
} v3_snr_lfid;

typedef enum {
    V3_SNR_LVSYNCT_NORMAL,
    V3_SNR_LVSYNCT_SHARE,
    V3_SNR_LVSYNCT_HCONNECT,
    V3_SNR_LVSYNCT_END
} v3_snr_lvsynct;

typedef enum {
    V3_SNR_LWDR_NONE,
    V3_SNR_LWDR_2F,
    V3_SNR_LWDR_3F,
    V3_SNR_LWDR_4F,
    V3_SNR_LWDR_DOL2F,
    V3_SNR_LWDR_DOL3F,
    V3_SNR_LWDR_DOL4F,
    V3_SNR_LWDR_END
} v3_snr_lwdr;

typedef enum {
    V3_SNR_MWDR_NONE,
    V3_SNR_MWDR_VC,
    V3_SNR_MWDR_DT,
    V3_SNR_MWDR_DOL,
    V3_SNR_MWDR_END
} v3_snr_mwdr;

typedef struct {
    v3_snr_lfid type;
    unsigned char outputFil;
} v3_snr_fid;

typedef struct {
    v3_snr_lvsynct type;
    unsigned short hBlank1;
    unsigned short hBlank2; 
} v3_snr_lvsync;

typedef struct {
    v3_common_dim dest;
    v3_common_prec prec;
    v3_snr_lwdr wdr;
    int syncSavOn;
    v3_snr_lvsync vsync;
    v3_snr_fid fid;
    int dataBeOn;
    int syncBeOn;
    // Value -1 signifies a lane is disabled
    short laneId[V3_SNR_LVDS_LANE_NUM];
    /* Each lane has two virtual channel, each has four params
       If syncSavOn is false: SOF, EOF, SOL, EOL
       If syncSavOn is true: invalid sav, invalid eav, valid sav, valid eav  */
    unsigned short syncCode[V3_SNR_LVDS_LANE_NUM * V3_SNR_WDR_VC_NUM * 4];
} v3_snr_lvds;

typedef struct {
    v3_common_prec prec;
    v3_snr_mwdr mode;
    // Value -1 signifies a lane is disabled
    short laneId[V3_SNR_MIPI_LANE_NUM];
    union {
        short wdrVcType[V3_SNR_WDR_VC_NUM];
    };
} v3_snr_mipi;

typedef struct {
    unsigned int device;
    v3_snr_input input;
    union {
        v3_snr_mipi mipi;
        v3_snr_lvds lvds;
    };
} v3_snr_dev;

typedef struct {
    void *handle;

    int (*fnRegisterCallback)(void);
    int (*fnUnRegisterCallback)(void);
} v3_snr_drv_impl;


/* v3_vb.h */


typedef enum {
    V3_VB_JPEG_MASK = 0x1,
    V3_VB_USERINFO_MASK = 0x2,
    V3_VB_ISPINFO_MASK = 0x4,
    V3_VB_ISPSTAT_MASK = 0x8,
    V3_VB_DNG_MASK = 0x10
} v3_vb_supl;

typedef struct {
    unsigned int count;
    struct {
        unsigned int blockSize;
        unsigned int blockCnt;
        char heapName[16];
        int rempVirtOn;
    } comm[16];
} v3_vb_pool;

typedef struct {
    void *handle;

    int (*fnConfigPool)(v3_vb_pool *config);
    int (*fnConfigSupplement)(v3_vb_supl *value);
    int (*fnExit)(void);
    int (*fnInit)(void);
} v3_vb_impl;


/* v3_vi.h */


typedef enum {
    V3_VI_INTF_BT656,
    V3_VI_INTF_BT601,
    V3_VI_INTF_DIGITAL_CAMERA,
    V3_VI_INTF_BT1120_STANDARD,
    V3_VI_INTF_BT1120_INTERLEAVED,
    V3_VI_INTF_MIPI,
    V3_VI_INTF_LVDS,
    V3_VI_INTF_HISPI,
    V3_VI_INTF_END
} v3_vi_intf;

typedef enum {
    V3_VI_REPHASE_NONE,
    V3_VI_REPHASE_SKIP1_2,
    V3_VI_REPHASE_SKIP1_3,
    V3_VI_REPHASE_BINNING1_2,
    V3_VI_REPHASE_BINNING1_3,
    V3_VI_REPHASE_END
} v3_vi_rephase;

typedef enum {
    V3_VI_SEQ_VUVU = 0,
    V3_VI_SEQ_UVUV,
    V3_VI_SEQ_UYVY = 0,
    V3_VI_SEQ_VYUY,
    V3_VI_SEQ_YUYV,
    V3_VI_SEQ_YVYU,
    V3_VI_SEQ_END
} v3_vi_seq;

typedef enum {
    V3_VI_WORK_1MULTIPLEX,
    V3_VI_WORK_2MULTIPLEX,
    V3_VI_WORK_4MULTIPLEX
} v3_vi_work;

typedef struct {
    v3_common_rect capt;
    v3_common_dim dest;
    // Accepts values from 0-2 (top, bottom, both)
    int field;
    v3_common_pixfmt pixFmt;
    v3_common_compr compress;
    int mirror;
    int flip;
    int srcFps;
    int dstFps;
} v3_vi_chn;

typedef struct {
    unsigned int hsyncFront;
    unsigned int hsyncWidth;
    unsigned int hsyncBack;
    unsigned int vsyncFront;
    unsigned int vsyncWidth;
    unsigned int vsyncBack;
    // Next three are valid on interlace mode
    // and define even-frame timings
    unsigned int vsyncIntrlFront;
    unsigned int vsyncIntrlWidth;
    unsigned int vsyncIntrlBack;
} v3_vi_timing;

typedef struct {
    int vsyncPulse;
    int vsyncInv;
    int hsyncPulse;
    int hsyncInv;
    int vsyncValid;
    int vsyncValidInv;
    v3_vi_timing timing;
} v3_vi_sync;

typedef struct {
    v3_vi_intf intf;
    v3_vi_work work;
    unsigned int cmpntMask[2];
    int progressiveOn;
    int adChn[4];
    v3_vi_seq seq;
    v3_vi_sync sync;
    // Accepts values from 0-2 (bypass, isp, raw)
    int dataPath;
    int rgbModeOn;
    int dataRevOn;
    v3_common_rect rect;
    v3_common_dim bayerSize;
    int bayerComprOn;
    v3_vi_rephase hsyncBayerReph;
    v3_vi_rephase vsyncBayerReph;
} v3_vi_dev;

typedef struct {
    void *handle;

    int (*fnDisableDevice)(int device);
    int (*fnEnableDevice)(int device);
    int (*fnSetDeviceConfig)(int device, v3_vi_dev *config);

    int (*fnDisableChannel)(int channel);
    int (*fnEnableChannel)(int channel);
    int (*fnSetChannelConfig)(int channel, v3_vi_chn *config);
} v3_vi_impl;


/* v3_vpss.h */


#define V3_VPSS_CHN_NUM 11
#define V3_VPSS_GRP_NUM 32

typedef struct {
    int sharpOn;
    int borderEn;
    int mirror;
    int flip;
    int srcFps;
    int dstFps;
    v3_common_bord border;
} v3_vpss_chn;

typedef struct {
    v3_common_dim dest;
    v3_common_pixfmt pixFmt;
    int enhOn;
    int dciOn;
    int nredOn;
    int histEn;
    // Accepts values from 0-2 (auto, off, on)
    int interlMode;
    int sharpOn;
} v3_vpss_grp;

typedef struct {
    int userModeOn;
    v3_common_dim dest;
    int doubleOn;
    v3_common_pixfmt pixFmt;
    v3_common_compr compress;
} v3_vpss_mode;

typedef struct {
    void *handle;

    int (*fnCreateGroup)(int group, v3_vpss_grp *config);
    int (*fnDestroyGroup)(int group);
    int (*fnResetGroup)(int group);
    int (*fnSetGroupConfig)(int group, v3_vpss_grp *config);
    int (*fnStartGroup)(int group);
    int (*fnStopGroup)(int group);

    int (*fnDisableChannel)(int group, int channel);
    int (*fnEnableChannel)(int group, int channel);
    int (*fnSetChannelConfig)(int group, int channel, v3_vpss_chn *config);
    int (*fnSetChannelMode)(int group, int channel, v3_vpss_mode *config);
} v3_vpss_impl;


/* v3_isp.h */


extern int (*fnISP_AlgRegisterDehaze)(int);
extern int (*fnISP_AlgRegisterDrc)(int);

typedef struct {
    int id;
    char libName[20];
} v3_isp_alg;

typedef struct {
    v3_common_rect capt;
    float framerate;
    v3_common_bayer bayer;
} v3_isp_dev;

typedef struct {
    void *handle, *handleAwb, *handleAe, *handleDefog, *handleIrAuto;

    int (*fnExit)(int device);
    int (*fnInit)(int device);
    int (*fnMemInit)(int device);
    int (*fnRun)(int device);

    int (*fnSetDeviceConfig)(int device, v3_isp_dev *config);
    int (*fnSetWDRMode)(int device, v3_common_wdr *mode);

    int (*fnRegisterAE)(int device, v3_isp_alg *library);
    int (*fnRegisterAWB)(int device, v3_isp_alg *library);
    int (*fnUnregisterAE)(int device, v3_isp_alg *library);
    int (*fnUnregisterAWB)(int device, v3_isp_alg *library);
} v3_isp_impl;


/* v3_venc.h */


#define V3_VENC_CHN_NUM 16

typedef enum {
    V3_VENC_CODEC_JPEG = 26,
    V3_VENC_CODEC_H264 = 96,
    V3_VENC_CODEC_H265 = 265,
    V3_VENC_CODEC_MJPG = 1002,
    V3_VENC_CODEC_END
} v3_venc_codec;

typedef enum {
    V3_VENC_GOPMODE_NORMALP,
    V3_VENC_GOPMODE_DUALP,
    V3_VENC_GOPMODE_SMARTP,
    V3_VENC_GOPMODE_BIPREDB,
    V3_VENC_GOPMODE_LOWDELAYB,
    V3_VENC_GOPMODE_END
} v3_venc_gopmode;

typedef enum {
    V3_VENC_NALU_H264_BSLICE,
    V3_VENC_NALU_H264_PSLICE,
    V3_VENC_NALU_H264_ISLICE,
    V3_VENC_NALU_H264_IDRSLICE = 5,
    V3_VENC_NALU_H264_SEI,
    V3_VENC_NALU_H264_SPS,
    V3_VENC_NALU_H264_PPS,
    V3_VENC_NALU_H264_END
} v3_venc_nalu_h264;

typedef enum {
    V3_VENC_NALU_H265_BSLICE,
    V3_VENC_NALU_H265_PSLICE,
    V3_VENC_NALU_H265_ISLICE,
    V3_VENC_NALU_H265_IDRSLICE = 19,
    V3_VENC_NALU_H265_VPS = 32,
    V3_VENC_NALU_H265_SPS,
    V3_VENC_NALU_H265_PPS,
    V3_VENC_NALU_H265_SEI = 39,
    V3_VENC_NALU_H265_END
} v3_venc_nalu_h265;

typedef enum {
    V3_VENC_NALU_MJPG_ECS = 5,
    V3_VENC_NALU_MJPG_APP,
    V3_VENC_NALU_MJPG_VDO,
    V3_VENC_NALU_MJPG_PIC,
    V3_VENC_NALU_MJPG_END
} v3_venc_nalu_mjpg;

typedef enum {
    V3_VENC_RATEMODE_H264CBR = 1,
    V3_VENC_RATEMODE_H264VBR,
    V3_VENC_RATEMODE_H264AVBR,
    V3_VENC_RATEMODE_H264QVBR,
    V3_VENC_RATEMODE_H264QP,
    V3_VENC_RATEMODE_H264QPMAP,
    V3_VENC_RATEMODE_MJPGCBR,
    V3_VENC_RATEMODE_MJPGVBR,
    V3_VENC_RATEMODE_MJPGQP,
    V3_VENC_RATEMODE_H265CBR,
    V3_VENC_RATEMODE_H265VBR,
    V3_VENC_RATEMODE_H265AVBR,
    V3_VENC_RATEMODE_H265QVBR,
    V3_VENC_RATEMODE_H265QP,
    V3_VENC_RATEMODE_H265QPMAP,
    V3_VENC_RATEMODE_END
} v3_venc_ratemode;

typedef struct {
    v3_common_dim maxPic;
    unsigned int bufSize;
    unsigned int profile;
    int byFrame;
    v3_common_dim pic;
} v3_venc_attr_h26x;

typedef struct {
    v3_common_dim maxPic;
    unsigned int bufSize;
    int byFrame;
    v3_common_dim pic;
} v3_venc_attr_mjpg;

typedef struct {
    v3_common_dim maxPic;
    unsigned int bufSize;
    int byFrame;
    v3_common_dim pic;
    int dcfThumbs;
} v3_venc_attr_jpg;

typedef struct {
    v3_venc_codec codec;
    union {
        v3_venc_attr_h26x h264;
        v3_venc_attr_mjpg mjpg;
        v3_venc_attr_jpg  jpg;
        v3_venc_attr_h26x h265;
    };
} v3_venc_attrib;

typedef struct {
    unsigned int gop;
    unsigned int statTime;
    unsigned int srcFps;
    unsigned int dstFps;
    unsigned int bitrate;
    unsigned int avgLvl;
} v3_venc_rate_h26xcbr;

typedef struct {
    unsigned int gop;
    unsigned int statTime;
    unsigned int srcFps;
    unsigned int dstFps;
    unsigned int maxBitrate;
    unsigned int maxQual;
    unsigned int minQual;
    unsigned int minIQual;
} v3_venc_rate_h26xvbr;

typedef struct {
    unsigned int gop;
    unsigned int statTime;
    unsigned int srcFps;
    unsigned int dstFps;
    unsigned int bitrate;
} v3_venc_rate_h26xxvbr;

typedef struct {
    unsigned int gop;
    unsigned int srcFps;
    unsigned int dstFps;
    unsigned int interQual;
    unsigned int predQual;
    unsigned int bipredQual;
} v3_venc_rate_h26xqp;

typedef struct {
    unsigned int gop;
    unsigned int statTime;
    unsigned int srcFps;
    unsigned int dstFps;
    // Accepts values from 0-2 (mean QP, min QP, max QP)
    unsigned int qpMapMode;
    unsigned int predQual;
    unsigned int bipredQual;
} v3_venc_rate_h26xqpmap;

typedef struct {
    unsigned int statTime;
    unsigned int srcFps;
    unsigned int dstFps;
    unsigned int bitrate;
    unsigned int avgLvl;
} v3_venc_rate_mjpgcbr;

typedef struct {
    unsigned int statTime;
    unsigned int srcFps;
    unsigned int dstFps;
    unsigned int maxBitrate;
    unsigned int maxQual;
    unsigned int minQual;
} v3_venc_rate_mjpgvbr;

typedef struct {
    unsigned int srcFps;
    unsigned int dstFps;
    unsigned int quality;
} v3_venc_rate_mjpgqp;
typedef struct {
    v3_venc_ratemode mode;
    union {
        v3_venc_rate_h26xcbr h264Cbr;
        v3_venc_rate_h26xvbr h264Vbr;
        v3_venc_rate_h26xxvbr h264Avbr;
        v3_venc_rate_h26xxvbr h264Qvbr;
        v3_venc_rate_h26xqp h264Qp;
        v3_venc_rate_h26xqpmap h264QpMap;
        v3_venc_rate_mjpgcbr mjpgCbr;
        v3_venc_rate_mjpgvbr mjpgVbr;
        v3_venc_rate_mjpgqp mjpgQp;
        v3_venc_rate_h26xcbr h265Cbr;
        v3_venc_rate_h26xvbr h265Vbr;
        v3_venc_rate_h26xxvbr h265Avbr;
        v3_venc_rate_h26xxvbr h265Qvbr;
        v3_venc_rate_h26xqp h265Qp;
        v3_venc_rate_h26xqpmap h265QpMap;
    };
    void *extend;
} v3_venc_rate;

typedef struct {
    int ipQualDelta;
} v3_venc_gop_normalp;

typedef struct {
    unsigned int spInterv;
    int spQualDelta;
    int ipQualDelta;
} v3_venc_gop_dualp;

typedef struct {
    unsigned int bgInterv;
    int bgQualDelta;
    int viQualDelta;
} v3_venc_gop_smartp;

typedef struct {
    unsigned int bFrameNum;
    int bgQualDelta;
    int ipQualDelta;
} v3_venc_gop_bipredb;

typedef struct {
    v3_venc_gopmode mode;
    union {
        v3_venc_gop_normalp normalP;
        v3_venc_gop_dualp dualP;
        v3_venc_gop_smartp smartP;
        v3_venc_gop_bipredb bipredB;
    };
} v3_venc_gop;

typedef struct {
    v3_venc_attrib attrib;
    v3_venc_rate rate;
    v3_venc_gop gop;
} v3_venc_chn;

typedef struct {
    unsigned int quality;
    unsigned char qtLuma[64];
    unsigned char qtChromaBlue[64];
    unsigned char qtChromaRed[64];
    unsigned int mcuPerEcs;
} v3_venc_jpg;

typedef union {
    v3_venc_nalu_h264 h264Nalu;
    v3_venc_nalu_mjpg mjpgNalu;
    v3_venc_nalu_h265 h265Nalu;
} v3_venc_nalu;

typedef struct {
    v3_venc_nalu packType;
    unsigned int offset;
    unsigned int length;
} v3_venc_packinfo;

typedef struct {
    unsigned int addr;
    unsigned char *data;
    unsigned int length;
    unsigned long long timestamp;
    int endFrame;
    v3_venc_nalu naluType;
    unsigned int offset;
    unsigned int packNum;
    v3_venc_packinfo packetInfo[8];
} v3_venc_pack;

typedef struct {
    unsigned int leftPics;
    unsigned int leftBytes;
    unsigned int leftFrames;
    unsigned int curPacks;
    unsigned int leftRecvPics;
    unsigned int leftEncPics;
} v3_venc_stat;

typedef struct {
    unsigned int size;
    unsigned int iMb16x16;
    unsigned int iMb8x8;
    unsigned int pMb16;
    unsigned int pMb8;
    unsigned int pMb4;
    unsigned int refSliceType;
    unsigned int refType;
    unsigned int updAttrCnt;
    unsigned int startQual;
    unsigned int meanQual;
    int pSkip;
} v3_venc_strminfo_h264;

typedef struct {
    unsigned int size;
    unsigned int iCu64x64;
    unsigned int iCu32x32;
    unsigned int iCu16x16;
    unsigned int iCu8x8;
    unsigned int pCu32x32;
    unsigned int pCu16x16;
    unsigned int pCu8x8;
    unsigned int pCu4x4;
    unsigned int refType;
    unsigned int updAttrCnt;
    unsigned int startQual;
    unsigned int meanQual;
    int pSkip;
} v3_venc_strminfo_h265;

typedef struct {
    unsigned int size;
    unsigned int updAttrCnt;
    unsigned int quality;
} v3_venc_strminfo_mjpg;

typedef struct {
    int sseOn;
    unsigned int sseVal;
} v3_venc_sseinfo;

typedef struct {
    unsigned int residualBitNum;
    unsigned int headBitNum;
    unsigned int madiVal;
    unsigned int madpVal;
    double psnrVal;
    unsigned int mseLcuCnt;
    unsigned int mseSum;
    v3_venc_sseinfo sseInfo[8];
    unsigned int qualHstgrm[52];
} v3_venc_strmadvinfo_h26x;

typedef struct {
    unsigned int reserved;
} v3_venc_strmadvinfo_mjpg;

typedef struct {
    v3_venc_pack *packet;
    unsigned int count;
    unsigned int sequence;
    union {
        v3_venc_strminfo_h264 h264Info;
        v3_venc_strminfo_mjpg mjpgInfo;
        v3_venc_strminfo_h265 h265Info;
    };
    union {
        v3_venc_strmadvinfo_h26x h264aInfo;
        v3_venc_strmadvinfo_mjpg mjpgaInfo;
        v3_venc_strmadvinfo_h26x h265aInfo;
    };
} v3_venc_strm;

typedef struct {
    void *handle;

    int (*fnCreateChannel)(int channel, v3_venc_chn *config);
    int (*fnGetChannelConfig)(int channel, v3_venc_chn *config);
    int (*fnDestroyChannel)(int channel);
    int (*fnResetChannel)(int channel);
    int (*fnSetChannelConfig)(int channel, v3_venc_chn *config);
    int (*fnSetColorToGray)(int channel, int *active);

    int (*fnFreeDescriptor)(int channel);
    int (*fnGetDescriptor)(int channel);

    int (*fnGetJpegParam)(int channel, v3_venc_jpg *param);
    int (*fnSetJpegParam)(int channel, v3_venc_jpg *param);

    int (*fnFreeStream)(int channel, v3_venc_strm *stream);
    int (*fnGetStream)(int channel, v3_venc_strm *stream, unsigned int timeout);

    int (*fnQuery)(int channel, v3_venc_stat* stats);

    int (*fnRequestIdr)(int channel, int instant);
    int (*fnStartReceiving)(int channel);
    int (*fnStartReceivingEx)(int channel, int *count);
    int (*fnStopReceiving)(int channel);
} v3_venc_impl;


const unsigned int layout_facts[] = {sizeof(void *), sizeof(v3_sys_bind), sizeof(v3_sys_ver), sizeof(v3_snr_dev), sizeof(v3_snr_lvds), __builtin_offsetof(v3_snr_dev,input), __builtin_offsetof(v3_snr_dev,lvds.laneId), __builtin_offsetof(v3_snr_dev,lvds.syncCode), sizeof(v3_vb_pool), sizeof(((v3_vb_pool *)0)->comm[0]), sizeof(v3_vi_dev), sizeof(v3_vi_chn), sizeof(v3_vpss_grp), sizeof(v3_vpss_chn), sizeof(v3_vpss_mode), sizeof(v3_isp_alg), sizeof(v3_isp_dev), sizeof(v3_venc_chn), sizeof(v3_venc_pack), sizeof(v3_venc_strm), sizeof(v3_venc_stat), V3_SNR_INPUT_HISPI, V3_SNR_INPUT_LVDS, V3_BAYER_GR, V3_PIXFMT_YUV420SP};

#endif
