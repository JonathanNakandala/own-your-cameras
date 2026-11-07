/* Original private compatibility implementation. SPDX-License-Identifier: MIT */
#include <stddef.h>
#include <stdint.h>
#ifndef COMPAT_TEST
_Static_assert(sizeof(unsigned long) == 4 && sizeof(void *) == 4, "ARM32 ABI");
extern int *__errno_location(void);
extern unsigned long pthread_self(void);
extern int prctl(int, ...);
#define ERRNO (*__errno_location())
#define SELF pthread_self
#define CONTROL prctl
#endif
/* Captured before libc initialization; entries are kernel-owned initial-stack words. */
static const uintptr_t *auxiliary;
static size_t auxiliary_count;
int qualification_capture_stack(const uintptr_t *stack) {
    auxiliary = NULL;
    auxiliary_count = 0;
    if (!stack || stack[0] > 65536) return -1;
    size_t i = (size_t)stack[0] + 1;
    if (stack[i] != 0) return -1;
    ++i;
    size_t env = 0;
    while (stack[i] != 0 && env < 65536) { ++i; ++env; }
    if (env == 65536) return -1;
    ++i;
    for (size_t count = 0; count < 256; ++count) {
        if (stack[i + 2 * count] == 0) {
            auxiliary = stack + i;
            auxiliary_count = count;
            return 0;
        }
    }
    return -1;
}
unsigned long getauxval(unsigned long kind) {
    for (size_t i = 0; i < auxiliary_count; ++i) {
        if (auxiliary[2 * i] == kind) return (unsigned long)auxiliary[2 * i + 1];
    }
    ERRNO = 2; /* ENOENT: no such entry, distinct from a present zero value. */
    return 0;
}
int pthread_setname_np(unsigned long thread, const char *name) {
    if (!name) return 22; /* EINVAL; no libc call and no errno change. */
    size_t length = 0;
    while (length < 16 && name[length] != 0) ++length;
    if (length == 16) return 34; /* ERANGE, unlike prctl's silent truncation. */
    if (thread != SELF()) return 95; /* ENOTSUP: intentionally no foreign-thread support. */
    int previous = ERRNO;
    int status = CONTROL(15, (unsigned long)(uintptr_t)name, 0UL, 0UL, 0UL);
    int result = status == 0 ? 0 : ERRNO;
    ERRNO = previous;
    return result;
}
