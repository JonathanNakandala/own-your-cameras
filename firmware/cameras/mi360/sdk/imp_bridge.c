/* Direct video adapter for the exact Mi360 shipped IMP 1.1.2 library.
 * ABI provenance: T31/1.1.2/zh/imp at ingenic-headers commit
 * 5bacc0f6ed70221081fe3f0aaa5e7f47aff89deb, corroborated against shipped
 * libimp SHA256 347b7a602b535541fa9594d100aee3bb87d6df35dec60fa9e16a94cabcffbca4.
 * The adapter checks the qualified ABI before owning media resources.
 */
#include "imp_bridge.h"

#include <dlfcn.h>
#include <imp_abi.h>
#include <errno.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>


_Static_assert(sizeof(void *) == 4, "requires O32 pointers");
_Static_assert(sizeof(bool) == 1, "C bool");
_Static_assert(sizeof(IMPCell) == 12, "sizeof IMPCell");
_Static_assert(_Alignof(IMPCell) == 4, "align IMPCell");
_Static_assert(sizeof(IMPI2CInfo) == 28, "sizeof IMPI2CInfo");
_Static_assert(_Alignof(IMPI2CInfo) == 4, "align IMPI2CInfo");
_Static_assert(sizeof(IMPSPIInfo) == 36, "sizeof IMPSPIInfo");
_Static_assert(_Alignof(IMPSPIInfo) == 4, "align IMPSPIInfo");
_Static_assert(sizeof(IMPSensorInfo) == 80, "sizeof IMPSensorInfo");
_Static_assert(_Alignof(IMPSensorInfo) == 4, "align IMPSensorInfo");
_Static_assert(sizeof(IMPFSChnCrop) == 20, "sizeof IMPFSChnCrop");
_Static_assert(_Alignof(IMPFSChnCrop) == 4, "align IMPFSChnCrop");
_Static_assert(sizeof(IMPFSChnScaler) == 12, "sizeof IMPFSChnScaler");
_Static_assert(_Alignof(IMPFSChnScaler) == 4, "align IMPFSChnScaler");
_Static_assert(sizeof(IMPFSChnAttr) == 80, "sizeof IMPFSChnAttr");
_Static_assert(_Alignof(IMPFSChnAttr) == 4, "align IMPFSChnAttr");
_Static_assert(sizeof(IMPEncoderCropCfg) == 20, "sizeof IMPEncoderCropCfg");
_Static_assert(_Alignof(IMPEncoderCropCfg) == 4, "align IMPEncoderCropCfg");
_Static_assert(sizeof(IMPEncoderEncAttr) == 44, "sizeof IMPEncoderEncAttr");
_Static_assert(_Alignof(IMPEncoderEncAttr) == 4, "align IMPEncoderEncAttr");
_Static_assert(sizeof(IMPEncoderGopAttr) == 24, "sizeof IMPEncoderGopAttr");
_Static_assert(_Alignof(IMPEncoderGopAttr) == 4, "align IMPEncoderGopAttr");
_Static_assert(sizeof(IMPEncoderAttrFixQP) == 2, "sizeof IMPEncoderAttrFixQP");
_Static_assert(_Alignof(IMPEncoderAttrFixQP) == 2, "align IMPEncoderAttrFixQP");
_Static_assert(sizeof(IMPEncoderAttrCbr) == 24, "sizeof IMPEncoderAttrCbr");
_Static_assert(_Alignof(IMPEncoderAttrCbr) == 4, "align IMPEncoderAttrCbr");
_Static_assert(sizeof(IMPEncoderAttrVbr) == 28, "sizeof IMPEncoderAttrVbr");
_Static_assert(_Alignof(IMPEncoderAttrVbr) == 4, "align IMPEncoderAttrVbr");
_Static_assert(sizeof(IMPEncoderAttrCappedVbr) == 32, "sizeof IMPEncoderAttrCappedVbr");
_Static_assert(_Alignof(IMPEncoderAttrCappedVbr) == 4, "align IMPEncoderAttrCappedVbr");
_Static_assert(sizeof(IMPEncoderAttrRcMode) == 36, "sizeof IMPEncoderAttrRcMode");
_Static_assert(_Alignof(IMPEncoderAttrRcMode) == 4, "align IMPEncoderAttrRcMode");
_Static_assert(sizeof(IMPEncoderFrmRate) == 8, "sizeof IMPEncoderFrmRate");
_Static_assert(_Alignof(IMPEncoderFrmRate) == 4, "align IMPEncoderFrmRate");
_Static_assert(sizeof(IMPEncoderRcAttr) == 44, "sizeof IMPEncoderRcAttr");
_Static_assert(_Alignof(IMPEncoderRcAttr) == 4, "align IMPEncoderRcAttr");
_Static_assert(sizeof(IMPEncoderChnAttr) == 112, "sizeof IMPEncoderChnAttr");
_Static_assert(_Alignof(IMPEncoderChnAttr) == 4, "align IMPEncoderChnAttr");
_Static_assert(sizeof(IMPEncoderChnStat) == 24, "sizeof IMPEncoderChnStat");
_Static_assert(_Alignof(IMPEncoderChnStat) == 4, "align IMPEncoderChnStat");
_Static_assert(sizeof(IMPEncoderNalType) == 4, "sizeof IMPEncoderNalType");
_Static_assert(_Alignof(IMPEncoderNalType) == 4, "align IMPEncoderNalType");
_Static_assert(sizeof(IMPEncoderPack) == 32, "sizeof IMPEncoderPack");
_Static_assert(_Alignof(IMPEncoderPack) == 8, "align IMPEncoderPack");
_Static_assert(sizeof(IMPEncoderStream) == 24, "sizeof IMPEncoderStream");
_Static_assert(_Alignof(IMPEncoderStream) == 4, "align IMPEncoderStream");
_Static_assert(offsetof(IMPSensorInfo, name) == 0, "offset IMPSensorInfo.name");
_Static_assert(offsetof(IMPSensorInfo, cbus_type) == 32, "offset IMPSensorInfo.cbus_type");
_Static_assert(offsetof(IMPSensorInfo, i2c) == 36, "offset IMPSensorInfo.i2c");
_Static_assert(offsetof(IMPSensorInfo, i2c.addr) == 56, "offset IMPSensorInfo.i2c.addr");
_Static_assert(offsetof(IMPSensorInfo, i2c.i2c_adapter_id) == 60, "offset IMPSensorInfo.i2c.i2c_adapter_id");
_Static_assert(offsetof(IMPSensorInfo, rst_gpio) == 72, "offset IMPSensorInfo.rst_gpio");
_Static_assert(offsetof(IMPSensorInfo, pwdn_gpio) == 74, "offset IMPSensorInfo.pwdn_gpio");
_Static_assert(offsetof(IMPSensorInfo, power_gpio) == 76, "offset IMPSensorInfo.power_gpio");
_Static_assert(offsetof(IMPFSChnAttr, picWidth) == 0, "offset IMPFSChnAttr.picWidth");
_Static_assert(offsetof(IMPFSChnAttr, picHeight) == 4, "offset IMPFSChnAttr.picHeight");
_Static_assert(offsetof(IMPFSChnAttr, pixFmt) == 8, "offset IMPFSChnAttr.pixFmt");
_Static_assert(offsetof(IMPFSChnAttr, crop) == 12, "offset IMPFSChnAttr.crop");
_Static_assert(offsetof(IMPFSChnAttr, scaler) == 32, "offset IMPFSChnAttr.scaler");
_Static_assert(offsetof(IMPFSChnAttr, outFrmRateNum) == 44, "offset IMPFSChnAttr.outFrmRateNum");
_Static_assert(offsetof(IMPFSChnAttr, outFrmRateDen) == 48, "offset IMPFSChnAttr.outFrmRateDen");
_Static_assert(offsetof(IMPFSChnAttr, nrVBs) == 52, "offset IMPFSChnAttr.nrVBs");
_Static_assert(offsetof(IMPFSChnAttr, type) == 56, "offset IMPFSChnAttr.type");
_Static_assert(offsetof(IMPFSChnAttr, fcrop) == 60, "offset IMPFSChnAttr.fcrop");
_Static_assert(offsetof(IMPEncoderChnAttr, encAttr) == 0, "offset IMPEncoderChnAttr.encAttr");
_Static_assert(offsetof(IMPEncoderChnAttr, rcAttr) == 44, "offset IMPEncoderChnAttr.rcAttr");
_Static_assert(offsetof(IMPEncoderChnAttr, gopAttr) == 88, "offset IMPEncoderChnAttr.gopAttr");
_Static_assert(offsetof(IMPEncoderChnAttr, rcAttr.attrRcMode.rcMode) == 44, "offset IMPEncoderChnAttr.rcAttr.attrRcMode.rcMode");
_Static_assert(offsetof(IMPEncoderChnAttr, rcAttr.attrRcMode.attrCappedVbr.iMinQP) == 58, "offset IMPEncoderChnAttr.rcAttr.attrRcMode.attrCappedVbr.iMinQP");
_Static_assert(offsetof(IMPEncoderChnAttr, rcAttr.attrRcMode.attrCappedVbr.iMaxQP) == 60, "offset IMPEncoderChnAttr.rcAttr.attrRcMode.attrCappedVbr.iMaxQP");
_Static_assert(offsetof(IMPEncoderChnAttr, rcAttr.outFrmRate.frmRateNum) == 80, "offset IMPEncoderChnAttr.rcAttr.outFrmRate.frmRateNum");
_Static_assert(offsetof(IMPEncoderChnAttr, rcAttr.outFrmRate.frmRateDen) == 84, "offset IMPEncoderChnAttr.rcAttr.outFrmRate.frmRateDen");
_Static_assert(offsetof(IMPEncoderEncAttr, uLevel) == 4, "offset IMPEncoderEncAttr.uLevel");
_Static_assert(offsetof(IMPEncoderEncAttr, uTier) == 5, "offset IMPEncoderEncAttr.uTier");
_Static_assert(offsetof(IMPEncoderEncAttr, uWidth) == 6, "offset IMPEncoderEncAttr.uWidth");
_Static_assert(offsetof(IMPEncoderEncAttr, uHeight) == 8, "offset IMPEncoderEncAttr.uHeight");
_Static_assert(offsetof(IMPEncoderEncAttr, ePicFormat) == 12, "offset IMPEncoderEncAttr.ePicFormat");
_Static_assert(offsetof(IMPEncoderEncAttr, eEncOptions) == 16, "offset IMPEncoderEncAttr.eEncOptions");
_Static_assert(offsetof(IMPEncoderEncAttr, eEncTools) == 20, "offset IMPEncoderEncAttr.eEncTools");
_Static_assert(offsetof(IMPEncoderEncAttr, crop) == 24, "offset IMPEncoderEncAttr.crop");
_Static_assert(offsetof(IMPEncoderPack, offset) == 0, "offset IMPEncoderPack.offset");
_Static_assert(offsetof(IMPEncoderPack, length) == 4, "offset IMPEncoderPack.length");
_Static_assert(offsetof(IMPEncoderPack, timestamp) == 8, "offset IMPEncoderPack.timestamp");
_Static_assert(offsetof(IMPEncoderPack, frameEnd) == 16, "offset IMPEncoderPack.frameEnd");
_Static_assert(offsetof(IMPEncoderPack, nalType) == 20, "offset IMPEncoderPack.nalType");
_Static_assert(offsetof(IMPEncoderPack, sliceType) == 24, "offset IMPEncoderPack.sliceType");
_Static_assert(offsetof(IMPEncoderStream, phyAddr) == 0, "offset IMPEncoderStream.phyAddr");
_Static_assert(offsetof(IMPEncoderStream, virAddr) == 4, "offset IMPEncoderStream.virAddr");
_Static_assert(offsetof(IMPEncoderStream, streamSize) == 8, "offset IMPEncoderStream.streamSize");
_Static_assert(offsetof(IMPEncoderStream, pack) == 12, "offset IMPEncoderStream.pack");
_Static_assert(offsetof(IMPEncoderStream, packCount) == 16, "offset IMPEncoderStream.packCount");
_Static_assert(offsetof(IMPEncoderStream, seq) == 20, "offset IMPEncoderStream.seq");
_Static_assert(IMP_ENC_PROFILE_AVC_BASELINE == 66, "enum IMP_ENC_PROFILE_AVC_BASELINE");
_Static_assert(IMP_ENC_PROFILE_AVC_MAIN == 77, "enum IMP_ENC_PROFILE_AVC_MAIN");
_Static_assert(IMP_ENC_PROFILE_AVC_HIGH == 100, "enum IMP_ENC_PROFILE_AVC_HIGH");
_Static_assert(IMP_ENC_PROFILE_HEVC_MAIN == 16777217, "enum IMP_ENC_PROFILE_HEVC_MAIN");
_Static_assert(IMP_ENC_PROFILE_JPEG == 67108864, "enum IMP_ENC_PROFILE_JPEG");
_Static_assert(IMP_ENC_RC_MODE_CAPPED_VBR == 4, "enum IMP_ENC_RC_MODE_CAPPED_VBR");
_Static_assert(IMP_ENC_PIC_FORMAT_420_8BITS == 392, "enum IMP_ENC_PIC_FORMAT_420_8BITS");
_Static_assert(PIX_FMT_NV12 == 10, "enum PIX_FMT_NV12");
_Static_assert(DEV_ID_FS == 0, "enum DEV_ID_FS");
_Static_assert(DEV_ID_ENC == 1, "enum DEV_ID_ENC");
_Static_assert(FS_PHY_CHANNEL == 0, "enum FS_PHY_CHANNEL");
_Static_assert(IMP_ENC_GOP_CTRL_MODE_DEFAULT == 2, "enum IMP_ENC_GOP_CTRL_MODE_DEFAULT");
_Static_assert(sizeof(struct t31_video_spec) == 40 && _Alignof(struct t31_video_spec) == 4,
               "video spec FFI layout");
_Static_assert(offsetof(struct t31_video_spec, main_width) == 0 &&
               offsetof(struct t31_video_spec, main_height) == 4 &&
               offsetof(struct t31_video_spec, sub_width) == 8 &&
               offsetof(struct t31_video_spec, sub_height) == 12 &&
               offsetof(struct t31_video_spec, main_profile) == 16 &&
               offsetof(struct t31_video_spec, sub_profile) == 20 &&
               offsetof(struct t31_video_spec, main_fps) == 24 &&
               offsetof(struct t31_video_spec, sub_fps) == 28 &&
               offsetof(struct t31_video_spec, main_bitrate) == 32 &&
               offsetof(struct t31_video_spec, sub_bitrate) == 36, "video spec offsets");

#define MAX_FRAME_BYTES (1024U * 1024U)
#define MAX_RING_BYTES (8U * 1024U * 1024U)
#define MAX_PACKS 256U
#define POLL_MILLISECONDS 100U
#define SDK_LIBRARY "/mnt/usr/lib/libimp.so"
#define MATH_LIBRARY "/lib/libm.so.0"
#define LOGGER_LIBRARY "/mnt/usr/lib/libalog.so"
_Static_assert(RTLD_NOW == 2 && RTLD_GLOBAL == 4 && RTLD_LOCAL == 0,
               "qualified MIPS uClibc loader flags");

/* Use the declarations from the pinned headers as the function-pointer types. */
#define SDK_FUNCTIONS(X) \
    X(IMP_ISP_Open) \
    X(IMP_ISP_Close) \
    X(IMP_ISP_AddSensor) \
    X(IMP_ISP_DelSensor) \
    X(IMP_ISP_EnableSensor) \
    X(IMP_ISP_DisableSensor) \
    X(IMP_System_Init) \
    X(IMP_System_Exit) \
    X(IMP_ISP_EnableTuning) \
    X(IMP_ISP_DisableTuning) \
    X(IMP_ISP_Tuning_SetContrast) \
    X(IMP_ISP_Tuning_SetSharpness) \
    X(IMP_ISP_Tuning_SetSaturation) \
    X(IMP_ISP_Tuning_SetBrightness) \
    X(IMP_ISP_Tuning_SetISPRunningMode) \
    X(IMP_FrameSource_CreateChn) \
    X(IMP_FrameSource_SetChnAttr) \
    X(IMP_FrameSource_DestroyChn) \
    X(IMP_FrameSource_EnableChn) \
    X(IMP_FrameSource_DisableChn) \
    X(IMP_Encoder_CreateGroup) \
    X(IMP_Encoder_DestroyGroup) \
    X(IMP_Encoder_SetDefaultParam) \
    X(IMP_Encoder_SetMaxStreamCnt) \
    X(IMP_Encoder_CreateChn) \
    X(IMP_Encoder_DestroyChn) \
    X(IMP_Encoder_RegisterChn) \
    X(IMP_Encoder_UnRegisterChn) \
    X(IMP_System_Bind) \
    X(IMP_System_UnBind) \
    X(IMP_Encoder_StartRecvPic) \
    X(IMP_Encoder_StopRecvPic) \
    X(IMP_Encoder_PollingStream) \
    X(IMP_Encoder_GetStream) \
    X(IMP_Encoder_ReleaseStream)

struct sdk_api {
#define DECLARE_SDK(name) __typeof__(&name) name;
    SDK_FUNCTIONS(DECLARE_SDK)
#undef DECLARE_SDK
};

struct video {
    unsigned int channel_id;
    unsigned int width;
    unsigned int height;
    unsigned int fps;
    struct video *sub;
    /* The snapshot channel, registered in the sub channel's encoder group. */
    struct video *jpeg;
    void *library;
    void *math_library;
    void *logger_library;
    struct sdk_api sdk;
    IMPSensorInfo sensor;
    IMPCell source;
    IMPCell sink;
    IMPEncoderStream stream;
    bool isp_open;
    bool sensor_added;
    bool sensor_enabled;
    bool system_initialized;
    bool tuning_enabled;
    bool fs_created;
    bool fs_enabled;
    bool group_created;
    bool encoder_created;
    bool encoder_registered;
    bool bound;
    bool receiving;
    bool stream_held;
    bool acquisition_uncertain;
    bool failed;
    bool closed;
};

/* One SDK owner in this process. No libc-specific pthread ABI crosses the shim. */
static struct video *owner;
static unsigned int call_busy;
_Static_assert(sizeof(call_busy) == 4, "native MIPS atomic guard");
_Static_assert(TX_SENSOR_CONTROL_INTERFACE_I2C == 1, "sensor I2C bus enum");
_Static_assert(IMPISP_RUNNING_MODE_DAY == 0, "ISP day mode enum");

static void clear_error(char *error, uint32_t cap)
{
    if (error != NULL && cap != 0)
        error[0] = '\0';
}

static void append_error(char *error, uint32_t cap, const char *op, int code,
                         const char *detail)
{
    uint32_t used = 0;
    if (error == NULL || cap == 0)
        return;
    while (used < cap && error[used] != '\0')
        ++used;
    if (used == cap)
        return;
    (void)snprintf(error + used, cap - used, "%s%s(code=%d)%s%s",
                   used == 0 ? "" : "; ", op, code,
                   detail != NULL ? ": " : "", detail != NULL ? detail : "");
}

static int enter_call(char *error, uint32_t cap)
{
    clear_error(error, cap);
    if (__atomic_exchange_n(&call_busy, 1U, __ATOMIC_ACQUIRE) != 0) {
        append_error(error, cap, "video_call", EBUSY, "concurrent call rejected");
        return -1;
    }
    return 0;
}

static void leave_call(void)
{
    __atomic_store_n(&call_busy, 0U, __ATOMIC_RELEASE);
}

static bool pipeline_empty(const struct video *v)
{
    return !v->acquisition_uncertain && (v->sub == NULL || pipeline_empty(v->sub)) &&
           (v->jpeg == NULL || pipeline_empty(v->jpeg)) &&
           !v->fs_created && !v->fs_enabled && !v->group_created &&
           !v->encoder_created && !v->encoder_registered && !v->bound &&
           !v->receiving && !v->stream_held;
}

static bool sdk_owners_empty(const struct video *v)
{
    return !v->acquisition_uncertain && pipeline_empty(v) && !v->tuning_enabled && !v->sensor_enabled &&
           !v->sensor_added && !v->isp_open && !v->system_initialized;
}

/*
 * Release is attempted once per successful GetStream, including validation errors.
 * On a release error the SDK's ownership is uncertain: retain the exact stream and
 * all parents, never retry the potentially destructive release on close.
 */
static int release_stream(struct video *v, char *error, uint32_t cap)
{
    int rc = v->sdk.IMP_Encoder_ReleaseStream(v->channel_id, &v->stream);
    if (rc != 0) {
        v->failed = true;
        append_error(error, cap, "IMP_Encoder_ReleaseStream", rc,
                     "POISONED: SDK stream ownership unresolved; reboot required");
        return -1;
    }
    v->stream_held = false;
    memset(&v->stream, 0, sizeof(v->stream));
    return 0;
}

/* A failed cleanup keeps its stage set; dependent resources remain intact. */
#define CLEAN_STAGE(field, call, operation) do { \
    int cleanup_rc = (call); \
    if (cleanup_rc == 0) { \
        v->field = false; \
    } else { \
        append_error(error, cap, operation, cleanup_rc, "cleanup failed"); \
    } \
} while (0)

static void cleanup(struct video *v, char *error, uint32_t cap)
{
    /* Acquisition failure atomicity is not certified. ISP_Open, for example,
     * stores an allocated global object before a failing driver open. Neither
     * guessing its paired teardown nor removing its shared parents is safe.
     */
    if (v->acquisition_uncertain) {
        v->failed = true;
        append_error(error, cap, "video_cleanup", EBUSY,
                     "POISONED: failed acquisition ownership uncertain; reboot required");
        return;
    }
    /* The JPEG channel leaves the main group before the main channel does. */
    if (v->jpeg != NULL)
        cleanup(v->jpeg, error, cap);
    if (v->sub != NULL)
        cleanup(v->sub, error, cap);
    /* A retained stream means Release already failed; do not repeat it. */
    if (v->receiving && !v->stream_held)
        CLEAN_STAGE(receiving, v->sdk.IMP_Encoder_StopRecvPic(v->channel_id),
                    "IMP_Encoder_StopRecvPic");
    if (v->fs_enabled && !v->stream_held)
        CLEAN_STAGE(fs_enabled, v->sdk.IMP_FrameSource_DisableChn(v->channel_id),
                    "IMP_FrameSource_DisableChn");
    if (v->bound && !v->receiving && !v->fs_enabled && !v->stream_held)
        CLEAN_STAGE(bound, v->sdk.IMP_System_UnBind(&v->source, &v->sink),
                    "IMP_System_UnBind");
    if (v->encoder_registered && !v->bound && !v->receiving && !v->stream_held)
        CLEAN_STAGE(encoder_registered, v->sdk.IMP_Encoder_UnRegisterChn(v->channel_id),
                    "IMP_Encoder_UnRegisterChn");
    if (v->encoder_created && !v->encoder_registered && !v->bound &&
        !v->receiving && !v->stream_held)
        CLEAN_STAGE(encoder_created, v->sdk.IMP_Encoder_DestroyChn(v->channel_id),
                    "IMP_Encoder_DestroyChn");
    if (v->group_created && !v->encoder_created && !v->encoder_registered &&
        !v->bound && !v->receiving && !v->stream_held)
        CLEAN_STAGE(group_created, v->sdk.IMP_Encoder_DestroyGroup(v->channel_id),
                    "IMP_Encoder_DestroyGroup");
    /* FS destruction is independent of an unbound encoder/group cleanup error. */
    if (v->fs_created && !v->fs_enabled && !v->bound)
        CLEAN_STAGE(fs_created, v->sdk.IMP_FrameSource_DestroyChn(v->channel_id),
                    "IMP_FrameSource_DestroyChn");

    /* Match shipped board_system_uninit ordering after every pipeline owner exits. */
    if (pipeline_empty(v)) {
        if (v->tuning_enabled)
            CLEAN_STAGE(tuning_enabled, v->sdk.IMP_ISP_DisableTuning(),
                        "IMP_ISP_DisableTuning");
        if (v->sensor_enabled && !v->tuning_enabled)
            CLEAN_STAGE(sensor_enabled, v->sdk.IMP_ISP_DisableSensor(),
                        "IMP_ISP_DisableSensor");
        if (v->sensor_added && !v->sensor_enabled && !v->tuning_enabled)
            CLEAN_STAGE(sensor_added, v->sdk.IMP_ISP_DelSensor(&v->sensor),
                        "IMP_ISP_DelSensor");
        if (v->isp_open && !v->sensor_added && !v->sensor_enabled &&
            !v->tuning_enabled)
            CLEAN_STAGE(isp_open, v->sdk.IMP_ISP_Close(), "IMP_ISP_Close");
        if (v->system_initialized && !v->isp_open && !v->sensor_added &&
            !v->sensor_enabled && !v->tuning_enabled)
            CLEAN_STAGE(system_initialized, v->sdk.IMP_System_Exit(),
                        "IMP_System_Exit");
    }
    if (!sdk_owners_empty(v)) {
        v->failed = true;
        append_error(error, cap, "video_cleanup", EBUSY,
                     "POISONED: SDK owners remain; reboot required");
    }
}
#undef CLEAN_STAGE

/* Exact strong UND import-to-GOT mapping from the pinned libimp ELF dynsym.
 * DT_PLTGOT=0xf4830, DT_MIPS_LOCAL_GOTNO=30, DT_MIPS_GOTSYM=472.
 * slot = 0xf4830 + 4 * (30 + dynsym_index - 472). Optional WEAK UND
 * imports are intentionally excluded. Validate providers and relocated slots:
 * the qualified legacy loader can return from RTLD_NOW with a null strong GOT.
 */
struct sdk_import { const char *name; uint32_t got_vaddr; };
static const struct sdk_import sdk_imports[] = {
    { "mmap", 0xf492cU },
    { "usleep", 0xf4930U },
    { "strtol", 0xf4950U },
    { "pthread_kill", 0xf4978U },
    { "strlen", 0xf4984U },
    { "memcpy", 0xf49d8U },
    { "getenv", 0xf4a18U },
    { "sprintf", 0xf4a20U },
    { "pthread_detach", 0xf4a68U },
    { "sem_post", 0xf4a98U },
    { "fopen", 0xf4ab0U },
    { "strerror", 0xf4ab4U },
    { "pthread_cancel", 0xf4ac4U },
    { "puts", 0xf4ae8U },
    { "stat", 0xf4b8cU },
    { "dlsym", 0xf4b94U },
    { "__fgetc_unlocked", 0xf4bacU },
    { "free", 0xf4bc0U },
    { "shm_open", 0xf4bd8U },
    { "syscall", 0xf4c2cU },
    { "pthread_mutex_lock", 0xf4c7cU },
    { "ftruncate", 0xf4c80U },
    { "pthread_cond_signal", 0xf4c94U },
    { "__assert", 0xf4cc4U },
    { "fputs", 0xf4cd0U },
    { "pthread_mutex_unlock", 0xf4d18U },
    { "__errno_location", 0xf4d60U },
    { "strtoull", 0xf4d8cU },
    { "IMP_Log_Get_Option", 0xf4d94U },
    { "sem_getvalue", 0xf4da8U },
    { "prctl", 0xf4e08U },
    { "fputc", 0xf4e6cU },
    { "pthread_cond_timedwait", 0xf4efcU },
    { "read", 0xf4f00U },
    { "imp_log_fun", 0xf4f14U },
    { "dlerror", 0xf4f44U },
    { "stdout", 0xf4f48U },
    { "sem_unlink", 0xf4f6cU },
    { "shm_unlink", 0xf4f7cU },
    { "calloc", 0xf4f90U },
    { "close", 0xf5028U },
    { "fread", 0xf504cU },
    { "pthread_mutex_init", 0xf5068U },
    { "fgets", 0xf50fcU },
    { "sem_trywait", 0xf5128U },
    { "__pthread_unregister_cancel", 0xf5194U },
    { "fprintf", 0xf5198U },
    { "dlclose", 0xf51b4U },
    { "putchar", 0xf5254U },
    { "sem_destroy", 0xf5258U },
    { "fgetc", 0xf5274U },
    { "__sigsetjmp", 0xf5280U },
    { "strncpy", 0xf528cU },
    { "sem_timedwait", 0xf5298U },
    { "fwrite", 0xf5310U },
    { "valloc", 0xf5330U },
    { "pthread_cond_destroy", 0xf5344U },
    { "pthread_mutexattr_init", 0xf5348U },
    { "printf", 0xf534cU },
    { "localtime_r", 0xf5370U },
    { "poll", 0xf5394U },
    { "pthread_mutex_destroy", 0xf53ecU },
    { "strchr", 0xf5438U },
    { "pthread_testcancel", 0xf5444U },
    { "perror", 0xf5488U },
    { "fscanf", 0xf5498U },
    { "sem_close", 0xf54f8U },
    { "pthread_cond_wait", 0xf5508U },
    { "munmap", 0xf5510U },
    { "pthread_mutexattr_settype", 0xf5564U },
    { "vfprintf", 0xf5578U },
    { "strstr", 0xf5594U },
    { "sem_open", 0xf55d0U },
    { "pow", 0xf55e4U },
    { "log10", 0xf55f4U },
    { "open", 0xf5600U },
    { "sleep", 0xf5630U },
    { "strdup", 0xf5650U },
    { "sqrt", 0xf56a0U },
    { "fclose", 0xf56a4U },
    { "pthread_create", 0xf56ccU },
    { "memalign", 0xf5720U },
    { "__cxa_atexit", 0xf5724U },
    { "clock_gettime", 0xf5728U },
    { "log10f", 0xf5738U },
    { "strcpy", 0xf5770U },
    { "pthread_exit", 0xf57a8U },
    { "strncmp", 0xf57b8U },
    { "__pthread_register_cancel", 0xf57c8U },
    { "pthread_join", 0xf57ccU },
    { "strcat", 0xf57dcU },
    { "memmove", 0xf5810U },
    { "dlopen", 0xf5818U },
    { "sem_init", 0xf582cU },
    { "malloc", 0xf586cU },
    { "sem_wait", 0xf58ccU },
    { "memset", 0xf58ecU },
    { "time", 0xf592cU },
    { "eventfd", 0xf5954U },
    { "strcmp", 0xf5974U },
    { "snprintf", 0xf59f8U },
    { "access", 0xf5a20U },
    { "getpagesize", 0xf5a40U },
    { "pthread_setschedparam", 0xf5a4cU },
    { "gettimeofday", 0xf5a60U },
    { "sscanf", 0xf5a90U },
    { "tan", 0xf5aacU },
    { "pthread_cond_init", 0xf5ae0U },
    { "pthread_self", 0xf5b04U },
    { "write", 0xf5b20U },
    { "realloc", 0xf5b2cU },
    { "abort", 0xf5b5cU },
    { "ioctl", 0xf5b8cU },
    { "stderr", 0xf5b9cU },
    { "memcmp", 0xf5ba4U },
    { "pthread_setcancelstate", 0xf5bb8U },
};
_Static_assert(sizeof(sdk_imports) / sizeof(sdk_imports[0]) == 116,
               "qualified SDK strong import count");

/* The host fault fixture supplies synthetic image/GOT reads; production uses the
 * exact ELF virtual addresses and actual relocated memory below. The Rust caller
 * verifies the entire DSO hash before entering this adapter.
 */
#ifndef BRIDGE_SDK_IMAGE_BASE
#define BRIDGE_SDK_IMAGE_BASE(open_address) ((open_address) - 0x7c8b4U)
#endif
#ifndef BRIDGE_SDK_GOT_VALUE
#define BRIDGE_SDK_GOT_VALUE(base, slot) \
    (*(const volatile uintptr_t *)((base) + (slot)))
#endif

static int verify_imports(struct video *v, char *error, uint32_t cap)
{
    uintptr_t open_address;
    uintptr_t image_base;
    memcpy(&open_address, &v->sdk.IMP_ISP_Open, sizeof(open_address));
    image_base = BRIDGE_SDK_IMAGE_BASE(open_address);
    /* Entire qualified GOT ends at VA0xf5c3c. Avoid wrapped or unaligned reads. */
    if (open_address < 0x7c8b4U || (image_base & 0xfffU) != 0 ||
        image_base > UINTPTR_MAX - 0xf5c3bU) {
        append_error(error, cap, "libimp image base", ENOEXEC,
                     "resolved ISP_Open differs from qualified ELF layout");
        return -1;
    }
    for (size_t i = 0; i < sizeof(sdk_imports) / sizeof(sdk_imports[0]); ++i) {
        char operation[128];
        void *symbol;
        const char *loader_error;
        (void)snprintf(operation, sizeof(operation), "SDK import %s", sdk_imports[i].name);
        (void)dlerror();
        /* Providers were loaded GLOBAL; they need not be in libimp's DT_NEEDED
         * dependency closure, so resolve the global scope explicitly.
         */
        symbol = dlsym(RTLD_DEFAULT, sdk_imports[i].name);
        loader_error = dlerror();
        if (symbol == NULL || loader_error != NULL) {
            append_error(error, cap, operation, ENOENT,
                         loader_error != NULL ? loader_error : "null strong symbol");
            return -1;
        }
        if (BRIDGE_SDK_GOT_VALUE(image_base, sdk_imports[i].got_vaddr) == 0) {
            append_error(error, cap, operation, ENOEXEC,
                         "relocated SDK GOT slot is null despite global provider");
            return -1;
        }
    }
    return 0;
}

/* Called only after every SDK owner is gone. Close SDK before its providers;
 * any failed unload retains the remaining dependency handles and owner storage.
 */
static int close_library(void **library, const char *operation,
                         char *error, uint32_t cap)
{
    if (*library != NULL) {
        int rc = dlclose(*library);
        if (rc != 0) {
            append_error(error, cap, operation, rc, dlerror());
            append_error(error, cap, "video_cleanup", EBUSY,
                         "POISONED: library retained; reboot required");
            return -1;
        }
        *library = NULL;
    }
    return 0;
}

static int dispose(struct video *v, char *error, uint32_t cap)
{
    if (close_library(&v->library, "dlclose libimp", error, cap) != 0 ||
        close_library(&v->logger_library, "dlclose libalog", error, cap) != 0 ||
        close_library(&v->math_library, "dlclose libm", error, cap) != 0)
        return -1;
    owner = NULL;
    free(v->jpeg);
    free(v->sub);
    free(v);
    return 0;
}

static int load_sdk(struct video *v, char *error, uint32_t cap)
{
    v->math_library = dlopen(MATH_LIBRARY, RTLD_NOW | RTLD_GLOBAL);
    if (v->math_library == NULL) {
        append_error(error, cap, "dlopen libm", -1, dlerror());
        return -1;
    }
    v->logger_library = dlopen(LOGGER_LIBRARY, RTLD_NOW | RTLD_GLOBAL);
    if (v->logger_library == NULL) {
        append_error(error, cap, "dlopen libalog", -1, dlerror());
        return -1;
    }
    v->library = dlopen(SDK_LIBRARY, RTLD_NOW | RTLD_LOCAL);
    if (v->library == NULL) {
        append_error(error, cap, "dlopen libimp", -1, dlerror());
        return -1;
    }
#define LOAD_SDK(name) do { \
    void *symbol; \
    const char *loader_error; \
    _Static_assert(sizeof(v->sdk.name) == sizeof(symbol), "function pointer ABI"); \
    (void)dlerror(); \
    symbol = dlsym(v->library, #name); \
    loader_error = dlerror(); \
    if (loader_error != NULL || symbol == NULL) { \
        append_error(error, cap, "dlsym " #name, -1, \
                     loader_error != NULL ? loader_error : "null symbol"); \
        return -1; \
    } \
    memcpy(&v->sdk.name, &symbol, sizeof(symbol)); \
} while (0);
    SDK_FUNCTIONS(LOAD_SDK)
#undef LOAD_SDK
    return verify_imports(v, error, cap);
}

#define INIT_STAGE(field, call, operation) do { \
    rc = (call); \
    if (rc != 0) { \
        v->acquisition_uncertain = true; \
        append_error(error, error_cap, operation, rc, \
                     "POISONED: acquisition ownership uncertain; reboot required"); \
        goto fail; \
    } \
    v->field = true; \
} while (0)
#define INIT_CALL(call, operation) do { \
    rc = (call); \
    if (rc != 0) { append_error(error, error_cap, operation, rc, NULL); goto fail; } \
} while (0)

static int configure_channel(struct video *v, unsigned int width, unsigned int height,
                             unsigned int profile, unsigned int fps, unsigned int bitrate,
                             char *error, uint32_t error_cap)
{
    IMPFSChnAttr fs;
    IMPEncoderChnAttr encoder;
    int rc;
    v->source = (IMPCell){ DEV_ID_FS, (int)v->channel_id, 0 };
    v->sink = (IMPCell){ DEV_ID_ENC, (int)v->channel_id, 0 };
    v->width = width;
    v->height = height;
    v->fps = fps;
    memset(&fs, 0, sizeof(fs));
    fs.picWidth = width;
    fs.picHeight = height;
    fs.pixFmt = PIX_FMT_NV12;
    /* Ordinary crop is AFTER scaling in this kernel (tisp_channel_attr_set,
     *0x14718 scaler ->0x14780 crop bounds). Keep the stock native-main record,
     * but never apply a1920x1080 post-scale crop to a smaller scaler output.
     * Scaled channels use the already qualified substream's no-crop pattern. */
    fs.crop = v->channel_id == 0 && width == 1920
        ? (IMPFSChnCrop){ 1, 0, 0, 1920, 1080 }
        : (IMPFSChnCrop){ 0, 0, 0, (int)width, (int)height };
    if (width != 1920)
        fs.scaler = (IMPFSChnScaler){ 1, (int)width, (int)height };
    fs.outFrmRateNum = fps;
    fs.outFrmRateDen = 1;
    fs.nrVBs = v->channel_id == 0 ? 1 : 2;
    fs.type = FS_PHY_CHANNEL;
    INIT_STAGE(fs_created, v->sdk.IMP_FrameSource_CreateChn(v->channel_id, &fs),
               "IMP_FrameSource_CreateChn");
    INIT_CALL(v->sdk.IMP_FrameSource_SetChnAttr(v->channel_id, &fs), "IMP_FrameSource_SetChnAttr");
    INIT_STAGE(group_created, v->sdk.IMP_Encoder_CreateGroup(v->channel_id), "IMP_Encoder_CreateGroup");
    memset(&encoder, 0, sizeof(encoder));
    /* One-second raw GOP at each channel's selected FPS. A fixed20-frame GOP
     * is fractional at rates such as7/13, which the duration control refuses. */
    INIT_CALL(v->sdk.IMP_Encoder_SetDefaultParam(&encoder,
              (IMPEncoderProfile)profile, IMP_ENC_RC_MODE_CAPPED_VBR,
              width, height, fps, 1, fps, 3, -1, bitrate), "IMP_Encoder_SetDefaultParam");
    encoder.rcAttr.attrRcMode.attrCappedVbr.iMinQP = 32;
    encoder.rcAttr.attrRcMode.attrCappedVbr.iMaxQP = 51;
    /* Shipped boardav sets one stream before CreateChn. The SDK default is two,
     * which wastes scarce reserved memory on this 16.5 MiB media region. */
    INIT_CALL(v->sdk.IMP_Encoder_SetMaxStreamCnt(v->channel_id, 1),
              "IMP_Encoder_SetMaxStreamCnt");
    INIT_STAGE(encoder_created, v->sdk.IMP_Encoder_CreateChn(v->channel_id, &encoder),
               "IMP_Encoder_CreateChn");
    INIT_STAGE(encoder_registered, v->sdk.IMP_Encoder_RegisterChn(v->channel_id, v->channel_id),
               "IMP_Encoder_RegisterChn");
    INIT_STAGE(bound, v->sdk.IMP_System_Bind(&v->source, &v->sink), "IMP_System_Bind");
    INIT_STAGE(fs_enabled, v->sdk.IMP_FrameSource_EnableChn(v->channel_id), "IMP_FrameSource_EnableChn");
    INIT_STAGE(receiving, v->sdk.IMP_Encoder_StartRecvPic(v->channel_id), "IMP_Encoder_StartRecvPic");
    return 0;
fail:
    return -1;
}

static bool valid_profile(uint32_t profile)
{
    return profile == IMP_ENC_PROFILE_AVC_BASELINE || profile == IMP_ENC_PROFILE_AVC_MAIN ||
           profile == IMP_ENC_PROFILE_AVC_HIGH;
}

static bool valid_spec(const struct t31_video_spec *s)
{
    return s != NULL &&
        ((s->main_width == 1920 && s->main_height == 1080) ||
         (s->main_width == 1280 && s->main_height == 720) ||
         (s->main_width == 640 && s->main_height == 360)) &&
        ((s->sub_width == 640 && s->sub_height == 360) ||
         (s->sub_width == 320 && s->sub_height == 180)) &&
        valid_profile(s->main_profile) && valid_profile(s->sub_profile) &&
        s->main_fps >= 1 && s->main_fps <= 20 && s->sub_fps >= 1 && s->sub_fps <= 20 &&
        s->main_bitrate >= 32 && s->main_bitrate <= 2048 &&
        s->sub_bitrate >= 32 && s->sub_bitrate <= 512;
}

static int open_graph(void **handle, const struct t31_video_spec *requested, bool dual,
                       char *error, uint32_t error_cap)
{
    struct video *v;
    struct t31_video_spec spec;
    int rc;
    if (enter_call(error, error_cap) != 0)
        return -1;
    if (handle == NULL) {
        append_error(error, error_cap, "video_open", EINVAL, "null handle output");
        leave_call();
        return -1;
    }
    *handle = NULL;
    if (requested == NULL) {
        append_error(error, error_cap, "video spec", EINVAL, "null graph specification");
        leave_call();
        return -1;
    }
    spec = *requested;
    if (!valid_spec(&spec)) {
        append_error(error, error_cap, "video spec", EINVAL, "unsupported graph specification");
        leave_call();
        return -1;
    }
    if (owner != NULL) {
        append_error(error, error_cap, "video_open", EBUSY,
                     owner->closed ? "POISONED: prior SDK owner retained; reboot required"
                                   : "another video handle is open");
        leave_call();
        return -1;
    }
    v = calloc(1, sizeof(*v));
    if (v == NULL) {
        append_error(error, error_cap, "calloc video", ENOMEM, NULL);
        leave_call();
        return -1;
    }
    if (dual) {
        v->sub = calloc(1, sizeof(*v->sub));
        if (v->sub == NULL) {
            free(v);
            append_error(error, error_cap, "calloc substream", ENOMEM, NULL);
            leave_call();
            return -1;
        }
        v->sub->channel_id = 1;
    }
    owner = v;
    if (load_sdk(v, error, error_cap) != 0)
        goto fail;

    /* Zeroed trailing sensor GPIO fields match stock; no raw GPIO writes. */
    memcpy(v->sensor.name, "sc2336", sizeof("sc2336"));
    v->sensor.cbus_type = TX_SENSOR_CONTROL_INTERFACE_I2C;
    memcpy(v->sensor.i2c.type, "sc2336", sizeof("sc2336"));
    v->sensor.i2c.addr = 0x30;
    v->sensor.i2c.i2c_adapter_id = 0;
    v->source = (IMPCell){ DEV_ID_FS, 0, 0 };
    v->sink = (IMPCell){ DEV_ID_ENC, 0, 0 };

    INIT_STAGE(isp_open, v->sdk.IMP_ISP_Open(), "IMP_ISP_Open");
    INIT_STAGE(sensor_added, v->sdk.IMP_ISP_AddSensor(&v->sensor), "IMP_ISP_AddSensor");
    INIT_STAGE(sensor_enabled, v->sdk.IMP_ISP_EnableSensor(), "IMP_ISP_EnableSensor");
    INIT_STAGE(system_initialized, v->sdk.IMP_System_Init(), "IMP_System_Init");
    INIT_STAGE(tuning_enabled, v->sdk.IMP_ISP_EnableTuning(), "IMP_ISP_EnableTuning");
    INIT_CALL(v->sdk.IMP_ISP_Tuning_SetContrast(128), "IMP_ISP_Tuning_SetContrast");
    INIT_CALL(v->sdk.IMP_ISP_Tuning_SetSharpness(128), "IMP_ISP_Tuning_SetSharpness");
    INIT_CALL(v->sdk.IMP_ISP_Tuning_SetSaturation(128), "IMP_ISP_Tuning_SetSaturation");
    INIT_CALL(v->sdk.IMP_ISP_Tuning_SetBrightness(128), "IMP_ISP_Tuning_SetBrightness");
    INIT_CALL(v->sdk.IMP_ISP_Tuning_SetISPRunningMode(IMPISP_RUNNING_MODE_DAY),
              "IMP_ISP_Tuning_SetISPRunningMode");

    if (configure_channel(v, spec.main_width, spec.main_height, spec.main_profile,
                          spec.main_fps, spec.main_bitrate, error, error_cap) != 0)
        goto fail;
    if (dual) {
        v->sub->sdk = v->sdk;
        if (configure_channel(v->sub, spec.sub_width, spec.sub_height, spec.sub_profile,
                              spec.sub_fps, spec.sub_bitrate, error, error_cap) != 0)
            goto fail;
    }
    *handle = v;
    leave_call();
    return 0;

fail:
    v->closed = true;
    /* If symbol resolution failed there are no completed stages, so no SDK calls. */
    cleanup(v, error, error_cap);
    if (sdk_owners_empty(v))
        (void)dispose(v, error, error_cap);
    leave_call();
    return -1;
}
int t31_video_open(void **handle, char *error, uint32_t error_cap)
{
    const struct t31_video_spec spec = T31_VIDEO_SPEC_DEFAULT;
    return open_graph(handle, &spec, false, error, error_cap);
}

int t31_video_open_dual(void **handle, char *error, uint32_t error_cap)
{
    const struct t31_video_spec spec = T31_VIDEO_SPEC_DEFAULT;
    return t31_video_open_spec(handle, &spec, error, error_cap);
}

int t31_video_open_spec(void **handle, const struct t31_video_spec *spec,
                        char *error, uint32_t error_cap)
{
    return open_graph(handle, spec, true, error, error_cap);
}

void *t31_video_sdk(void *handle)
{
    return handle != NULL && handle == owner && !owner->closed && !owner->failed
           ? owner->library : NULL;
}

int t31_video_night(void *handle, int night, char *error, uint32_t cap)
{
    int rc;
    if (enter_call(error, cap) != 0) return -1;
    if (handle == NULL || handle != owner || owner->closed || owner->failed ||
        (night != 0 && night != 1)) {
        append_error(error, cap, "video night mode", EINVAL, NULL);
        leave_call(); return -1;
    }
    rc = owner->sdk.IMP_ISP_Tuning_SetISPRunningMode(night ? IMPISP_RUNNING_MODE_NIGHT : IMPISP_RUNNING_MODE_DAY);
    if (rc != 0) append_error(error, cap, "IMP_ISP_Tuning_SetISPRunningMode", rc, NULL);
    leave_call(); return rc;
}

/* Snapshot channel: JPEG of the sub picture, encoder channel 2 in the sub channel's group (a main-size JPEG
 * buffer does not fit the 16.5 MiB media reservation: IMP_Alloc refused 949248 bytes), created
 * once after the graph and receiving only while a snapshot is wanted. Its failure leaves video alone. */
#define JPEG_CHANNEL 2U
#define JPEG_QP 25

static struct video *jpeg_of(void *handle, char *error, uint32_t error_cap)
{
    if (handle == NULL || handle != owner || owner->closed || owner->failed) {
        append_error(error, error_cap, "jpeg", EINVAL, "invalid, closed or failed video handle");
        return NULL;
    }
    return owner->jpeg;
}

int t31_jpeg_open(void *handle, char *error, uint32_t error_cap)
{
    struct video *v, *source;
    IMPEncoderChnAttr encoder;
    int rc;
    if (enter_call(error, error_cap) != 0)
        return -1;
    if (handle == NULL || handle != owner || owner->closed || owner->failed || owner->jpeg != NULL) {
        append_error(error, error_cap, "jpeg_open", EINVAL, "needs an open graph without a JPEG channel");
        leave_call();
        return -1;
    }
    source = owner->sub != NULL ? owner->sub : owner;
    v = calloc(1, sizeof(*v));
    if (v == NULL) {
        append_error(error, error_cap, "calloc jpeg", ENOMEM, NULL);
        leave_call();
        return -1;
    }
    v->channel_id = JPEG_CHANNEL;
    v->sdk = owner->sdk;
    v->width = source->width;
    v->height = source->height;
    owner->jpeg = v;
    memset(&encoder, 0, sizeof(encoder));
    INIT_CALL(v->sdk.IMP_Encoder_SetDefaultParam(&encoder, IMP_ENC_PROFILE_JPEG, IMP_ENC_RC_MODE_FIXQP,
              v->width, v->height, source->fps, 1, 0, 0, JPEG_QP, 0), "IMP_Encoder_SetDefaultParam jpeg");
    INIT_CALL(v->sdk.IMP_Encoder_SetMaxStreamCnt(v->channel_id, 1), "IMP_Encoder_SetMaxStreamCnt jpeg");
    /* A refused create allocated nothing (IMP_Alloc reports the refused DMA buffer before failing), and a
     * refused registration is undone at once: unlike the graph's stages, the snapshot channel's failure
     * must leave video and its later close alone. */
    INIT_CALL(v->sdk.IMP_Encoder_CreateChn(v->channel_id, &encoder), "IMP_Encoder_CreateChn jpeg");
    v->encoder_created = true;
    rc = v->sdk.IMP_Encoder_RegisterChn(source->channel_id, v->channel_id);
    if (rc != 0) {
        append_error(error, error_cap, "IMP_Encoder_RegisterChn jpeg", rc, NULL);
        if (v->sdk.IMP_Encoder_DestroyChn(v->channel_id) == 0)
            v->encoder_created = false;
        goto fail;
    }
    v->encoder_registered = true;
    leave_call();
    return 0;
fail:
    /* Nothing left acquired: forget the channel. Otherwise it stays for close to clean up, and
     * snapshots are refused. */
    if (!v->encoder_created && !v->acquisition_uncertain) {
        owner->jpeg = NULL;
        free(v);
    } else {
        v->failed = true;
    }
    leave_call();
    return -1;
}

int t31_jpeg_receive(void *handle, int on, char *error, uint32_t error_cap)
{
    struct video *v;
    int rc;
    if (enter_call(error, error_cap) != 0)
        return -1;
    v = jpeg_of(handle, error, error_cap);
    /* A failed channel may still be stopped, never started. */
    if (v == NULL || (on == 1 && v->failed) || !v->encoder_registered || v->stream_held ||
        (on != 0 && on != 1) || v->receiving == (on == 1)) {
        append_error(error, error_cap, "jpeg_receive", EINVAL, "JPEG channel unavailable or already in that state");
        leave_call();
        return -1;
    }
    rc = on ? v->sdk.IMP_Encoder_StartRecvPic(v->channel_id) : v->sdk.IMP_Encoder_StopRecvPic(v->channel_id);
    if (rc != 0) {
        v->failed = true;
        append_error(error, error_cap, on ? "IMP_Encoder_StartRecvPic jpeg" : "IMP_Encoder_StopRecvPic jpeg", rc, NULL);
    } else {
        v->receiving = on == 1;
    }
    leave_call();
    return rc == 0 ? 0 : -1;
}

#undef INIT_STAGE
#undef INIT_CALL

/* Bounds check a nonempty target address range without forming an overflowed ptr. */
static bool valid_range(uintptr_t address, uint32_t bytes)
{
    return address != 0 && bytes != 0 && (uintptr_t)(bytes - 1U) <= UINTPTR_MAX - address;
}

static int copy_frame(const IMPEncoderStream *stream, uint8_t *dst,
                      uint32_t capacity, uint32_t *copied, int64_t *pts,
                      char *error, uint32_t cap)
{
    uint32_t total = 0;
    int64_t first_timestamp = 0;
    uintptr_t base = (uintptr_t)stream->virAddr;
    uintptr_t packs = (uintptr_t)stream->pack;
    if (stream->streamSize == 0 || stream->streamSize > MAX_RING_BYTES ||
        stream->packCount == 0 || stream->packCount > MAX_PACKS ||
        !valid_range(base, stream->streamSize) ||
        packs % _Alignof(IMPEncoderPack) != 0 ||
        !valid_range(packs, stream->packCount * (uint32_t)sizeof(IMPEncoderPack))) {
        append_error(error, cap, "IMP_Encoder_GetStream metadata", EPROTO,
                     "invalid ring or pack array bounds");
        return -1;
    }
    /* First validate every pack and total size; never publish a partial frame. */
    for (uint32_t i = 0; i < stream->packCount; ++i) {
        const IMPEncoderPack *pack = &stream->pack[i];
        if (pack->length == 0)
            continue;
        if (pack->offset >= stream->streamSize || pack->length > stream->streamSize ||
            pack->length > capacity - total) {
            append_error(error, cap, "IMP_Encoder_GetStream pack", EOVERFLOW,
                         "invalid offset/length or complete frame exceeds destination");
            return -1;
        }
        if (total == 0)
            first_timestamp = pack->timestamp;
        total += pack->length;
    }
    if (total == 0) {
        append_error(error, cap, "IMP_Encoder_GetStream metadata", EPROTO,
                     "frame contains no encoded data");
        return -1;
    }
    total = 0;
    for (uint32_t i = 0; i < stream->packCount; ++i) {
        const IMPEncoderPack *pack = &stream->pack[i];
        uint32_t first;
        if (pack->length == 0)
            continue;
        first = stream->streamSize - pack->offset;
        if (first > pack->length)
            first = pack->length;
        memcpy(dst + total, (const void *)(base + pack->offset), first);
        if (pack->length > first)
            memcpy(dst + total + first, (const void *)base, pack->length - first);
        total += pack->length;
    }
    *copied = total;
    *pts = first_timestamp;
    return 0;
}

static int video_next(void *handle, unsigned int channel, unsigned int poll_ms, uint8_t *dst, uint32_t capacity,
                   uint32_t *length, int64_t *timestamp,
                   char *error, uint32_t error_cap)
{
    struct video *v;
    uint32_t copied = 0;
    int64_t pts = 0;
    int rc;
    if (enter_call(error, error_cap) != 0)
        return -1;
    if (length != NULL)
        *length = 0;
    if (timestamp != NULL)
        *timestamp = 0;
    if (handle == NULL || handle != owner || owner->closed || owner->failed) {
        append_error(error, error_cap, "video_next", EINVAL,
                     "invalid, closed or failed video handle");
        leave_call();
        return -1;
    }
    if (channel > JPEG_CHANNEL || (channel == 1 && owner->sub == NULL) ||
        (channel == JPEG_CHANNEL && (owner->jpeg == NULL || !owner->jpeg->encoder_registered))) {
        append_error(error, error_cap, "video channel", EINVAL, NULL);
        leave_call(); return -1;
    }
    v = channel == 0 ? owner : channel == 1 ? owner->sub : owner->jpeg;
    if (v->failed || v->closed) { leave_call(); return -1; }
    if (dst == NULL || capacity == 0 || capacity > MAX_FRAME_BYTES ||
        length == NULL || timestamp == NULL ||
        !valid_range((uintptr_t)dst, capacity)) {
        v->failed = true;
        append_error(error, error_cap, "video_next", EINVAL,
                     "requires valid outputs and a 1..1048576-byte destination");
        leave_call();
        return -1;
    }
    rc = v->sdk.IMP_Encoder_PollingStream(v->channel_id, poll_ms);
    if (rc != 0) {
        append_error(error, error_cap, "IMP_Encoder_PollingStream", rc,
                     "100 ms poll returned nonzero");
        leave_call();
        return 1;
    }
    memset(&v->stream, 0, sizeof(v->stream));
    rc = v->sdk.IMP_Encoder_GetStream(v->channel_id, &v->stream, false);
    if (rc != 0) {
        v->failed = true;
        v->acquisition_uncertain = true;
        append_error(error, error_cap, "IMP_Encoder_GetStream", rc,
                     "POISONED: failed acquisition ownership uncertain; reboot required");
        leave_call();
        return -1;
    }
    v->stream_held = true;
    rc = copy_frame(&v->stream, dst, capacity, &copied, &pts, error, error_cap);
    if (rc != 0)
        v->failed = true;
    /* Once, on every path after GetStream success, including a copy error. */
    if (release_stream(v, error, error_cap) != 0)
        rc = -1;
    if (rc == 0) {
        *length = copied;
        *timestamp = pts;
    }
    leave_call();
    return rc;
}

int t31_video_next(void *handle, uint8_t *dst, uint32_t capacity,
                   uint32_t *length, int64_t *timestamp,
                   char *error, uint32_t error_cap)
{
    return video_next(handle, 0, POLL_MILLISECONDS, dst, capacity, length, timestamp, error, error_cap);
}

int t31_video_next_channel(void *handle, unsigned int channel, uint8_t *dst, uint32_t capacity,
                           uint32_t *length, int64_t *timestamp, char *error, uint32_t error_cap)
{
    return video_next(handle, channel, 10, dst, capacity, length, timestamp, error, error_cap);
}

int t31_video_close(void *handle, char *error, uint32_t error_cap)
{
    struct video *v;
    int rc;
    if (enter_call(error, error_cap) != 0)
        return -1;
    if (handle == NULL || handle != owner || owner->closed) {
        append_error(error, error_cap, "video_close", EINVAL,
                     "invalid or already consumed video handle");
        leave_call();
        return -1;
    }
    v = owner;
    v->closed = true;
    cleanup(v, error, error_cap);
    rc = sdk_owners_empty(v) ? dispose(v, error, error_cap) : -1;
    leave_call();
    return rc;
}
