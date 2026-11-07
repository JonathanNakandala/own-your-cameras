#ifndef MI360_IMP_BRIDGE_H
#define MI360_IMP_BRIDGE_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/*
 * IMP 1.1.2 graph adapter. Caller must first verify the exact SDK
 * and provider hashes and establish exclusive ownership of the camera's media hardware.
 * libm and libalog load GLOBAL before the SDK; all strong SDK imports and their
 * actual GOT slots are checked before ISP initialization. No application logger
 * substitutes are exported. No audio, OSD, JPEG, IR or motor APIs are used. Calls are serialized internally;
 * overlapping calls fail rather than wait. Do not fork while a handle is open.
 *
 * open: 0 returns one handle; -1 leaves *handle NULL and describes the failure.
 * next: 0 copies one SDK frame into caller-owned dst; 1 is a nonzero result from
 *       the finite 100 ms SDK poll (its actual code is in error); -1 is fatal.
 *       capacity must be 1..1048576. On nonzero return length/timestamp are zero
 *       unless an overlapping call is rejected before touching those outputs.
 *       dst and output/error buffers must be valid, disjoint caller allocations.
 * close: consumes the handle exactly once, returns 0 on complete cleanup or -1.
 *
 * Never call next after a fatal result. Always close a successfully opened handle,
 * including after next fails. A cleanup/release failure can leave SDK owners alive:
 * close then retains internal storage/library and reports POISONED/reboot required.
 * A failed acquisition has uncertified failure atomicity: open/next then retains
 * shared graph parents and the library rather than guess a paired teardown.
 * A failed open unwinds only when ownership is certain; it can poison this process. Do not
 * retry an open after that diagnostic. External restoration/reboot is caller-owned.
 * Finite polling does not bound other SDK/kernel calls or guarantee driver health.
 */
int t31_video_open(void **handle, char *error, uint32_t error_cap);
int t31_video_next(void *handle, uint8_t *dst, uint32_t capacity,
                   uint32_t *length, int64_t *timestamp,
                   char *error, uint32_t error_cap);
int t31_video_close(void *handle, char *error, uint32_t error_cap);
/* Dual graph shares one System/ISP owner. Borrowed SDK outlives audio; close
 * every audio owner before video_close, and retain video after audio poison. */
int t31_video_open_dual(void **handle, char *error, uint32_t error_cap);
/* Fixed-width O32 FFI record: sizeof40, align4; each field at index*4.
 * AVC profile values are66 baseline,77 main,100 high. Bitrates are kbit/s.
 * Main geometry:1920x1080,1280x720,640x360; sub:640x360,320x180.
 * Integer channel FPS1..20; sensor mode remains the qualified native20fps.
 * Main bitrate32..2048, sub32..512: initial target kbit/s. The exact SDK helper
 * derives its maximum/allocated ceiling (approximately4/3 target); later scalar
 * bitrate changes remain bounded by that channel's actual RC getter ceiling.
 * Both encoders use capped VBR, minQP32/maxQP51, initial GOP equal to channel FPS (1s). Geometry,
 * profile and allocation changes require fully closing and reopening the graph.
 * Input validation precedes all allocation/loader/SDK calls. All combinations
 * are offline candidates; only the existing default has live acceptance so far.
 */
struct t31_video_spec {
    uint32_t main_width, main_height, sub_width, sub_height;
    uint32_t main_profile, sub_profile;
    uint32_t main_fps, sub_fps;
    uint32_t main_bitrate, sub_bitrate;
};
#define T31_VIDEO_SPEC_DEFAULT {1920,1080,640,360,66,66,20,20,800,240}
int t31_video_open_spec(void **handle, const struct t31_video_spec *spec,
                        char *error, uint32_t error_cap);
void *t31_video_sdk(void *handle);
int t31_video_night(void *handle, int night, char *error, uint32_t error_cap);
int t31_video_next_channel(void *handle, unsigned int channel, uint8_t *dst,
                           uint32_t capacity, uint32_t *length, int64_t *timestamp,
                           char *error, uint32_t error_cap);
/* Snapshot channel (video_next channel 2): created once after the graph, receiving only while on. */
int t31_jpeg_open(void *handle, char *error, uint32_t error_cap);
int t31_jpeg_receive(void *handle, int on, char *error, uint32_t error_cap);

#ifdef __cplusplus
}
#endif
#endif
