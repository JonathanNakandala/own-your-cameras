/* ARMv5 has no load/store-exclusive, so the compiler emits these libcalls for atomics (Zig's libc uses
 * them in its allocator). Linux provides __kuser_cmpxchg at a fixed address for exactly this case
 * (CONFIG_KUSER_HELPERS, present on the device's 3.18 kernel); musl's own ARMv5 atomics use it too. */
typedef int (*kuser_cmpxchg_t)(unsigned oldval, unsigned newval, volatile unsigned *ptr);
#define KUSER_CMPXCHG ((kuser_cmpxchg_t)0xffff0fc0)

/* The compiler refuses to define its own builtins by name; asm labels give the linker symbols it wants. */
unsigned kuser_cas4(volatile unsigned *p, unsigned expected, unsigned desired) __asm__("__sync_val_compare_and_swap_4");
unsigned char kuser_cas1(volatile unsigned char *p, unsigned char expected, unsigned char desired) __asm__("__sync_val_compare_and_swap_1");
unsigned char kuser_tas1(volatile unsigned char *p, unsigned char value) __asm__("__sync_lock_test_and_set_1");

unsigned kuser_cas4(volatile unsigned *p, unsigned expected, unsigned desired) {
    for (;;) {
        unsigned current = *p;
        if (current != expected) return current;
        if (KUSER_CMPXCHG(expected, desired, p) == 0) return expected;
    }
}

static unsigned char byte_cas(volatile unsigned char *p, unsigned char expected, unsigned char desired, int unconditional) {
    volatile unsigned *word = (volatile unsigned *)((unsigned)p & ~3u);
    unsigned shift = ((unsigned)p & 3u) * 8u;
    for (;;) {
        unsigned current = *word;
        unsigned char have = (unsigned char)((current >> shift) & 0xffu);
        if (!unconditional && have != expected) return have;
        unsigned next = (current & ~(0xffu << shift)) | ((unsigned)desired << shift);
        if (KUSER_CMPXCHG(current, next, word) == 0) return have;
    }
}

unsigned char kuser_cas1(volatile unsigned char *p, unsigned char expected, unsigned char desired) {
    return byte_cas(p, expected, desired, 0);
}

unsigned char kuser_tas1(volatile unsigned char *p, unsigned char value) {
    return byte_cas(p, 0, value, 1);
}
