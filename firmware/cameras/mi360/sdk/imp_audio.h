#ifndef MI360_IMP_AUDIO_H
#define MI360_IMP_AUDIO_H
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif

#define T31_AUDIO_CAPTURE_BYTES 3840U
#define T31_AUDIO_PLAY_BYTES 1280U
#define T31_AUDIO_POISONED (-2)

/* Exact Mi360 IMP ABI, signed little-endian PCM16 mono: capture 48 kHz in 40 ms frames of
 * 1920 samples; playback 16 kHz in 40 ms blocks of 640 samples.
 * The caller owns an initialized exclusive IMP System session and a loader
 * which has pinned/qualified all SDK imports and actual GOT bindings. sdk is a
 * borrowed dlopen handle, not a second SDK instance. No Init/Exit/dlopen/dlclose
 * occurs here. All handles/providers MUST outlive this audio owner, including
 * after POISONED. No video teardown/System_Exit while audio owners survive.
 *
 * No SDK effects (NS/AGC/AEC) are used. The external speaker amplifier is caller-owned.
 * Stock gain/volume defaults are requested, not independently verified: stock
 * SetGain can mask hardware failure and getters are cached.
 *
 * Initialize *handle=NULL before open; never overwrite an existing handle.
 * Caller allocations must be valid and disjoint for the full call; next dst
 * capacity is at least 3840, with writable length/timestamp and error buffers.
 * 0 success; 1 next has no frame / play has no free block; -1 terminal operation
 * failure/invalid arguments; -2 uncertain live SDK ownership, process poisoned.
 * Bound all caller retries: GetFrame's -1 is not a distinct EAGAIN contract.
 * next publishes length/timestamp only after release succeeds; dst can have
 * unpublished bytes after release failure. SDK-owned original frame released
 * exactly once. play takes exactly 1280 bytes and copies before SDK SendFrame.
 *
 * Calls are serialized internally by a nonwaiting gate. next/play after any
 * terminal hardware error are rejected until close. close consumes *handle,
 * attempts each independent known stage once, and retains uncertain owners.
 * No PollingFrame/Flush/sleep/retry loops. SDK mutex/ioctl/disable calls can still
 * block: there is NO honest hard wall-time bound in an in-process C adapter.
 * The media supervisor supplies deadline/escalation; never cancel an FFI thread
 * and unload its code. Do not fork while open. A poisoned failed open leaves
 * *handle NULL and blocks future opens; its shared owner must remain alive.
 */
int t31_audio_open(void **handle, void *sdk, char *error, uint32_t error_cap);
int t31_audio_next(void *handle, uint8_t *dst, uint32_t capacity,
                   uint32_t *length, int64_t *timestamp,
                   char *error, uint32_t error_cap);
int t31_audio_play(void *handle, const uint8_t *pcm, uint32_t length,
                   char *error, uint32_t error_cap);
int t31_audio_close(void **handle, char *error, uint32_t error_cap);
#ifdef __cplusplus
}
#endif
#endif
