/* Exact shipped Mi360 IMP1.1.2 audio. See docs/audio.md and private imp-audio
 * disassembly research. Headers pinned by sdk/headers.lock.json. */
#include "imp_audio.h"
#include <dlfcn.h>
#include <imp_abi.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

_Static_assert(sizeof(void *) == 4, "O32 pointers");
#define SIZE(T,N,A) _Static_assert(sizeof(T)==(N), #T " size"); \
    _Static_assert(_Alignof(T)==(A), #T " alignment")
#define OFF(T,F,N) _Static_assert(offsetof(T,F)==(N), #T "." #F)
SIZE(IMPAudioIOAttr,24,4);
OFF(IMPAudioIOAttr,samplerate,0); OFF(IMPAudioIOAttr,bitwidth,4);
OFF(IMPAudioIOAttr,soundmode,8); OFF(IMPAudioIOAttr,frmNum,12);
OFF(IMPAudioIOAttr,numPerFrm,16); OFF(IMPAudioIOAttr,chnCnt,20);
SIZE(IMPAudioFrame,32,8);
OFF(IMPAudioFrame,bitwidth,0); OFF(IMPAudioFrame,soundmode,4);
OFF(IMPAudioFrame,virAddr,8); OFF(IMPAudioFrame,phyAddr,12);
OFF(IMPAudioFrame,timeStamp,16); OFF(IMPAudioFrame,seq,24); OFF(IMPAudioFrame,len,28);
SIZE(IMPAudioIChnParam,8,4); OFF(IMPAudioIChnParam,usrFrmDepth,0); OFF(IMPAudioIChnParam,Rev,4);
SIZE(IMPAudioOChnState,12,4); OFF(IMPAudioOChnState,chnTotalNum,0);
OFF(IMPAudioOChnState,chnFreeNum,4); OFF(IMPAudioOChnState,chnBusyNum,8);
_Static_assert(BLOCK == 0 && NOBLOCK == 1, "block enum");
_Static_assert(AUDIO_SAMPLE_RATE_16000 == 16000 && AUDIO_SAMPLE_RATE_48000 == 48000 &&
               AUDIO_BIT_WIDTH_16 == 16 &&
               AUDIO_SOUND_MODE_MONO == 1, "PCM enums");

#define CORE(X) \
 X(IMP_AI_SetPubAttr) X(IMP_AI_Enable) X(IMP_AI_Disable) \
 X(IMP_AI_SetChnParam) X(IMP_AI_EnableChn) X(IMP_AI_DisableChn) \
 X(IMP_AI_GetFrame) X(IMP_AI_ReleaseFrame) X(IMP_AI_SetVol) X(IMP_AI_SetGain) \
 X(IMP_AO_SetPubAttr) X(IMP_AO_Enable) X(IMP_AO_Disable) \
 X(IMP_AO_EnableChn) X(IMP_AO_DisableChn) X(IMP_AO_SendFrame) \
 X(IMP_AO_QueryChnStat) X(IMP_AO_SetVol) X(IMP_AO_SetGain)
struct audio_api {
#define DECL(name) __typeof__(&name) name;
 CORE(DECL)
#undef DECL
};
struct audio {
    struct audio_api api;
    void *sdk;
    bool ai_dev, ai_ch, ao_dev, ao_ch;
    bool ai_dev_unknown, ai_ch_unknown, ao_dev_unknown, ao_ch_unknown, borrow_unknown;
    bool fatal, closing;
    IMPAudioFrame borrowed;
    uint32_t playback[T31_AUDIO_PLAY_BYTES / 4];
};
static struct audio *audio_owner;
static unsigned audio_gate;
static bool audio_poisoned;

static void message(char *error, uint32_t cap, const char *op, int code)
{
    if (error && cap) {
        size_t n = strlen(error);
        if (n < cap) (void)snprintf(error+n, cap-n, "%s%s=%d", n ? "; " : "", op, code);
    }
}
static int enter(char *error, uint32_t cap)
{
    if (__sync_lock_test_and_set(&audio_gate, 1)) return -1;
    if (error && cap) error[0] = 0;
    return 0;
}
static int leave(int result) { __sync_lock_release(&audio_gate); return result; }
static bool live(const struct audio *a)
{
    return a->ai_dev || a->ai_ch || a->ao_dev || a->ao_ch ||
        a->ai_dev_unknown || a->ai_ch_unknown || a->ao_dev_unknown || a->ao_ch_unknown ||
        a->borrow_unknown;
}
static void cleanup(struct audio *a, char *error, uint32_t cap)
{
    int rc;
    /* Never guess a second release after ReleaseFrame already failed. */
    if (!a->borrow_unknown && a->ai_ch && !a->ai_ch_unknown) {
        rc=a->api.IMP_AI_DisableChn(1,0);
        if (rc) { a->ai_ch_unknown=true; message(error,cap,"AI_DisableChn",rc); }
        else a->ai_ch=false;
    }
    if (a->ao_ch && !a->ao_ch_unknown) {
        rc=a->api.IMP_AO_DisableChn(0,0);
        if (rc) { a->ao_ch_unknown=true; message(error,cap,"AO_DisableChn",rc); }
        else a->ao_ch=false;
    }
    if (!a->ai_ch && !a->ai_ch_unknown && !a->borrow_unknown && a->ai_dev && !a->ai_dev_unknown) {
        rc=a->api.IMP_AI_Disable(1);
        if (rc) { a->ai_dev_unknown=true; message(error,cap,"AI_Disable",rc); }
        else a->ai_dev=false;
    }
    if (!a->ao_ch && !a->ao_ch_unknown && a->ao_dev && !a->ao_dev_unknown) {
        rc=a->api.IMP_AO_Disable(0);
        if (rc) { a->ao_dev_unknown=true; message(error,cap,"AO_Disable",rc); }
        else a->ao_dev=false;
    }
}
static int disposed(struct audio *a, char *error, uint32_t cap)
{
    if (live(a)) {
        audio_poisoned=true;
        message(error,cap,"POISONED retain shared System/providers; supervisor escalation required",-2);
        return T31_AUDIO_POISONED;
    }
    audio_owner=NULL;
    free(a);
    return 0;
}
static bool resolve(void *library, void *destination, size_t size, const char *name,
                    char *error, uint32_t cap)
{
    void *symbol=dlsym(library,name);
    if (!symbol || size != sizeof(symbol)) { message(error,cap,name,-1); return false; }
    memcpy(destination,&symbol,size);
    return true;
}

int t31_audio_open(void **handle, void *sdk, char *error, uint32_t cap)
{
    struct audio *a;
    /* The microphone runs at the codec's full 48 kHz; the speaker carries little above ~6.4 kHz
     * (docs/shared/camera-audio-response.md), so it stays at 16 kHz. */
    IMPAudioIOAttr ai={48000,16,1,25,1920,1}, ao={16000,16,1,25,640,1};
    IMPAudioIChnParam param={25,0};
    int rc;
    if (enter(error,cap)) return -1;
    if (!handle) return leave(-1);
    if (*handle) { message(error,cap,"open output must be NULL",-1); return leave(-1); }
    *handle=NULL;
    if (audio_poisoned || audio_owner) { message(error,cap,"audio owner unavailable",-2); return leave(audio_poisoned ? -2 : -1); }
    if (!sdk) { message(error,cap,"qualified SDK argument",-1); return leave(-1); }
    a=calloc(1,sizeof(*a));
    if (!a) { message(error,cap,"allocation",-1); return leave(-1); }
    a->sdk=sdk;
    audio_owner=a;
#define RESOLVE(name) if (!resolve(sdk,&a->api.name,sizeof(a->api.name),#name,error,cap)) goto fail;
    CORE(RESOLVE)
#undef RESOLVE
    /* Nonallocating setters are separate from acquisitions with unknown failure atomicity. */
#define CALL(expr) do { rc=(expr); if (rc) { message(error,cap,#expr,rc); goto fail; } } while (0)
#define ACQUIRE(expr,field) do { rc=(expr); if (rc) { a->field##_unknown=true; message(error,cap,#expr,rc); goto fail; } a->field=true; } while (0)
    CALL(a->api.IMP_AI_SetPubAttr(1,&ai));
    ACQUIRE(a->api.IMP_AI_Enable(1),ai_dev);
    CALL(a->api.IMP_AI_SetChnParam(1,0,&param));
    ACQUIRE(a->api.IMP_AI_EnableChn(1,0),ai_ch);
    CALL(a->api.IMP_AI_SetVol(1,0,55));
    CALL(a->api.IMP_AI_SetGain(1,0,20));
    CALL(a->api.IMP_AO_SetPubAttr(0,&ao));
    ACQUIRE(a->api.IMP_AO_Enable(0),ao_dev);
    ACQUIRE(a->api.IMP_AO_EnableChn(0,0),ao_ch);
    CALL(a->api.IMP_AO_SetVol(0,0,54));
    CALL(a->api.IMP_AO_SetGain(0,0,28));
#undef CALL
#undef ACQUIRE
    *handle=a;
    return leave(0);
fail:
    cleanup(a,error,cap);
    rc=disposed(a,error,cap);
    return leave(rc ? rc : -1);
}

static struct audio *usable(void *handle, char *error, uint32_t cap)
{
    if (!handle || handle != audio_owner) { message(error,cap,"invalid audio handle",-1); return NULL; }
    if (audio_poisoned || audio_owner->fatal || audio_owner->closing) {
        message(error,cap,"terminal audio state",-1); return NULL;
    }
    return audio_owner;
}
int t31_audio_next(void *handle, uint8_t *dst, uint32_t capacity,
                   uint32_t *length, int64_t *timestamp, char *error, uint32_t cap)
{
    struct audio *a;
    int rc, release;
    bool valid;
    int64_t stamp;
    if (enter(error,cap)) return -1;
    if (length) *length=0;
    if (timestamp) *timestamp=0;
    a=usable(handle,error,cap);
    if (!a || !dst || !length || !timestamp || capacity<T31_AUDIO_CAPTURE_BYTES) return leave(-1);
    memset(&a->borrowed,0,sizeof(a->borrowed));
    rc=a->api.IMP_AI_GetFrame(1,0,&a->borrowed,NOBLOCK);
    if (rc) {
        message(error,cap,"GetFrame (caller bounds retries)",rc);
        if (a->borrowed.bitwidth || a->borrowed.soundmode || a->borrowed.virAddr ||
            a->borrowed.phyAddr || a->borrowed.timeStamp || a->borrowed.seq || a->borrowed.len) {
            a->borrow_unknown=true; a->fatal=true; audio_poisoned=true; return leave(-2);
        }
        if (rc == -1) return leave(1);
        a->fatal=true; return leave(-1);
    }
    /* ReleaseFrame dereferences virAddr-20. Obvious malformed pointers cannot
     * safely be released; retain the parent rather than turn ABI corruption into a crash. */
    if ((uintptr_t)a->borrowed.virAddr < 4096 || (uintptr_t)a->borrowed.virAddr % 4 ||
        (uintptr_t)a->borrowed.virAddr > UINTPTR_MAX-T31_AUDIO_CAPTURE_BYTES) {
        a->borrow_unknown=true; a->fatal=true; audio_poisoned=true;
        message(error,cap,"GetFrame invalid release pointer; retain owner",-2); return leave(-2);
    }
    valid=a->borrowed.bitwidth==16 && a->borrowed.soundmode==1 &&
        a->borrowed.len==(int)T31_AUDIO_CAPTURE_BYTES;
    if (valid) memcpy(dst,a->borrowed.virAddr,T31_AUDIO_CAPTURE_BYTES);
    stamp=a->borrowed.timeStamp;
    release=a->api.IMP_AI_ReleaseFrame(1,0,&a->borrowed);
    if (release) {
        a->borrow_unknown=true; a->fatal=true; audio_poisoned=true;
        message(error,cap,"ReleaseFrame; do not release again",release); return leave(-2);
    }
    if (!valid) { a->fatal=true; message(error,cap,"invalid PCM frame released",-1); return leave(-1); }
    *length=T31_AUDIO_CAPTURE_BYTES; *timestamp=stamp;
    return leave(0);
}
int t31_audio_play(void *handle, const uint8_t *pcm, uint32_t length, char *error, uint32_t cap)
{
    struct audio *a;
    IMPAudioOChnState state={0};
    IMPAudioFrame frame={0};
    int rc;
    if (enter(error,cap)) return -1;
    a=usable(handle,error,cap);
    if (!a || !pcm || length!=T31_AUDIO_PLAY_BYTES) return leave(-1);
    rc=a->api.IMP_AO_QueryChnStat(0,0,&state);
    if (rc || state.chnTotalNum<2 || state.chnTotalNum>50 || state.chnFreeNum<0 ||
        state.chnBusyNum<0 || state.chnFreeNum>state.chnTotalNum ||
        state.chnBusyNum!=state.chnTotalNum-state.chnFreeNum) {
        a->fatal=true; message(error,cap,"QueryChnStat invalid/error",rc ? rc : -1); return leave(-1);
    }
    if (!state.chnFreeNum) return leave(1);
    memcpy(a->playback,pcm,length);
    frame.bitwidth=16; frame.soundmode=1; frame.virAddr=a->playback; frame.len=(int)length;
    rc=a->api.IMP_AO_SendFrame(0,0,&frame,NOBLOCK);
    if (rc) { a->fatal=true; message(error,cap,"SendFrame (not uniquely EAGAIN)",rc); return leave(-1); }
    return leave(0);
}
int t31_audio_close(void **handle, char *error, uint32_t cap)
{
    struct audio *a;
    int rc;
    if (enter(error,cap)) return -1;
    if (!handle || !*handle || *handle!=audio_owner || audio_owner->closing) return leave(-1);
    a=audio_owner; *handle=NULL; a->closing=true;
    cleanup(a,error,cap);
    rc=disposed(a,error,cap);
    return leave(rc);
}
