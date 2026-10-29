/* Exact sizes/placements around 8-byte granules and 512-byte bitmap blocks.
 * Run with gc-stress, NOT gc-every-alloc: garbage must remain until we collect. */
#include "abi.h"
#define REQUIRE(c) do { if (!(c)) return __LINE__; } while (0)
static unsigned char bytes[4096];
static const unsigned sizes[] = {0, 1, 7, 8, 9, 487, 488, 489, 495, 496, 497, 511, 512, 513, 1025};
static int contents(Object *p, unsigned n) {
    String *s = (String *)p;
    if (s->length != n) return 0;
    for (unsigned i = 0; i < n; ++i) if (s->data[i] != bytes[i]) return 0;
    return 1;
}
int lo_entry(void) {
    for (unsigned i = 0; i < sizeof(bytes); ++i) bytes[i] = (unsigned char)(i * 37 + 11);
    lo_runtime_init(); Frame f = {0, 3, {0, 0, 0}}; lo_push_frame(&f);
    for (unsigned start = 488; start <= 520; start += 8)
        for (unsigned k = 0; k < sizeof(sizes)/sizeof(sizes[0]); ++k) {
            f.roots[0] = f.roots[1] = 0; lo_gc_collect();
            Object *base = lo_string_new(bytes, start - offsetof(String, data));
            f.roots[0] = lo_string_new(bytes, sizes[k]);
            f.roots[1] = lo_string_new(bytes, 17);
            REQUIRE((uintptr_t)f.roots[0] - (uintptr_t)base == start);
            lo_gc_collect();
            REQUIRE(f.roots[0] == base); /* actually slid over the dead prefix */
            REQUIRE(contents(f.roots[0], sizes[k]) && contents(f.roots[1], 17));
            /* Repeatedly create holes before a newly retained string. */
            for (int round = 0; round < 20; ++round) {
                (void)lo_string_new(bytes, 113);
                f.roots[1] = lo_string_new(bytes, 17);
                lo_gc_collect();
                REQUIRE(contents(f.roots[0], sizes[k]) && contents(f.roots[1], 17));
            }
        }
    f.roots[0] = f.roots[1] = 0; lo_gc_collect();
    /* A single string fills the 4 KiB heap exactly, including its header. */
    f.roots[0] = lo_string_new(bytes, 4096 - offsetof(String, data));
    lo_gc_collect(); REQUIRE(contents(f.roots[0], 4096 - offsetof(String, data)));
    f.roots[0] = 0;
    /* Full dead heap must be reclaimed by the allocation retry path. */
    f.roots[0] = lo_string_new(bytes, 1);
    lo_gc_collect(); REQUIRE(contents(f.roots[0], 1));
    f.roots[0] = 0; lo_gc_collect(); lo_gc_collect();
    lo_pop_frame(); lo_runtime_shutdown(); return 0;
}
#ifndef __wasm__
int main(void) { return lo_entry(); }
#endif
