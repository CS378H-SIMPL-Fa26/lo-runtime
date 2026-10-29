/* Same ABI-level forced-collection/corruption cases on 32-bit WASM and x86-64.
 * Compile with -DCASE=n. No libc dependency, so WASM uses the course host ABI. */
#include "abi.h"

static const uint32_t offsets[] = { offsetof(Node, next) };
static const Class node_class = {
    "Node", 4, 0, sizeof(Node), offsets, 1, 0, 0
};
static const Node singleton = { { &node_class, 0, 0 }, 0 };
#define REQUIRE(condition) do { if (!(condition)) return __LINE__; } while (0)
#define NODE(p) ((Node *)(p))

int lo_entry(void) {
    lo_runtime_init();
    Frame outer = { 0, 3, { 0, 0, 0 } };
    Frame inner = { 0, 3, { 0, 0, 0 } };
    lo_push_frame(&outer);
    lo_push_frame(&inner);
    /* A 600-byte dead object crosses a bitmap block and forces overlapping
     * sliding moves of the survivors below it. */
    unsigned char bytes[600] = { 0 };
    (void)lo_string_new(bytes, sizeof(bytes));
    outer.roots[0] = lo_alloc(&node_class);
    outer.roots[1] = lo_alloc(&node_class);
    lo_gc_write_barrier(outer.roots[0], offsetof(Node, next), outer.roots[1]);
    lo_gc_write_barrier(outer.roots[1], offsetof(Node, next), outer.roots[0]);
    inner.roots[0] = outer.roots[0];

#if CASE == 1
    outer.roots[0]->class = (const Class *)(uintptr_t)1;
#elif CASE == 2
    NODE(outer.roots[0])->next = (Object *)((char *)outer.roots[0] + 8);
#elif CASE == 3
    inner.roots[0] = (Object *)(uintptr_t)1;
#elif CASE == 4
    /* No allocations follow the second node: the next byte is free. */
    *((unsigned char *)outer.roots[1] + ((sizeof(Node) + 7) & ~(size_t)7)) = 0;
#elif CASE == 5
    outer.roots[2] = lo_string_new((const unsigned char *)"ok", 2);
    ((String *)outer.roots[2])->length = UINT32_MAX;
#elif CASE == 6
    inner.roots[1] = (Object *)&singleton; /* Valid storage, but not registered. */
#elif CASE == 7
    inner.roots[1] = (Object *)((char *)outer.roots[1] + ((sizeof(Node) + 7) & ~(size_t)7));
#elif CASE == 8
    outer.roots[0]->gc_bits = 1;
#elif CASE == 9
    /* A genuinely live set larger than 4 KiB must still use ABI exit 137,
     * not a verifier failure or an endless allocation/collection retry. */
    for (int i = 0; i < 1000; ++i) {
        Object *fresh = lo_alloc(&node_class);
        lo_gc_write_barrier(fresh, offsetof(Node, next), outer.roots[0]);
        outer.roots[0] = fresh;
    }
#endif

    lo_gc_collect(); /* Negative cases must diagnose here, before unsafe marking. */
#if CASE != 0
    return 99; /* Reject a checker that silently accepts the corruption. */
#else
    REQUIRE(outer.roots[0] == inner.roots[0]);
    REQUIRE(NODE(outer.roots[0])->next == outer.roots[1]);
    REQUIRE(NODE(outer.roots[1])->next == outer.roots[0]);
    lo_gc_register_static((Object *)&singleton);
    inner.roots[1] = (Object *)&singleton;
    inner.roots[2] = (Object *)&LO_EMPTY_STRING;
    for (int i = 0; i < 2000; ++i) {
        (void)lo_alloc(&node_class); /* Exhaust a 4 KiB stress build repeatedly. */
        if (i % 47 == 0) lo_gc_collect();
    }
    REQUIRE(outer.roots[0] == inner.roots[0]);
    REQUIRE(NODE(outer.roots[0])->next == outer.roots[1]);
    REQUIRE(NODE(outer.roots[1])->next == outer.roots[0]);
    REQUIRE(inner.roots[1] == (Object *)&singleton);
    REQUIRE(inner.roots[2] == (Object *)&LO_EMPTY_STRING);

    /* Variable-sized UTF-8 strings survive forced GC and allocating runtime ops. */
    outer.roots[2] = lo_string_new((const unsigned char *)"a\xc3\xa9", 3);
    lo_gc_collect();
    inner.roots[0] = lo_string_reverse(outer.roots[2]);
    lo_gc_collect();
    inner.roots[2] = lo_string_new((const unsigned char *)"\xc3\xa9" "a", 3);
    REQUIRE(lo_string_compare(inner.roots[0], inner.roots[2]) == 0);
    outer.roots[2] = lo_string_concat(outer.roots[2], inner.roots[0]);
    lo_gc_collect();
    REQUIRE(((String *)outer.roots[2])->length == 6);
    lo_pop_frame();
    outer.roots[0] = outer.roots[1] = outer.roots[2] = 0;
    lo_gc_collect();
    lo_gc_collect(); /* Empty-heap collection is valid. */
    lo_pop_frame();
    lo_runtime_shutdown();
    return 0;
#endif
}

#ifndef __wasm__
int main(void) { return lo_entry(); }
#endif
