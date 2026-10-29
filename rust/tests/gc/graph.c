/* Deterministic model-based graph test. The oracle contains IDs/edges only,
 * never managed pointers, and independently computes reachability. */
#include "abi.h"
#ifndef SEED
#define SEED 1
#endif
#define N 32
#define STEPS 2000
#define REQUIRE(c) do { if (!(c)) return __LINE__; } while (0)
typedef struct Graph { Object header; Object *edge[2]; uint32_t id, payload; } Graph;
static const uint32_t offsets[] = { offsetof(Graph, edge[0]), offsetof(Graph, edge[1]) };
static const Class klass = { "Graph", 5, 0, sizeof(Graph), offsets, 2, 0, 0 };
static uint32_t rng = SEED;
static uint32_t random32(void) { rng ^= rng << 13; rng ^= rng >> 17; rng ^= rng << 5; return rng; }
static int edges[N][2], roots[3], alive[N];
static uint32_t ids[N], serial;
static Graph *actual[N]; /* Scratch, rebuilt after every possible collection. */
static void reachable(void) {
    for (int i = 0; i < N; ++i) alive[i] = 0;
    for (int r = 0; r < 3; ++r) if (roots[r] >= 0) alive[roots[r]] = 1;
    for (int pass = 0; pass < N; ++pass)
        for (int i = 0; i < N; ++i) if (alive[i])
            for (int e = 0; e < 2; ++e) if (edges[i][e] >= 0) alive[edges[i][e]] = 1;
}
static int bind(int slot, Object *p) {
    if (slot < 0) return p == 0;
    if (!p) return 0;
    Graph *g = (Graph *)p;
    if (g->header.class != &klass || g->id != ids[slot] || g->payload != (ids[slot] ^ 0xabc123u)) return 0;
    if (actual[slot] && actual[slot] != g) return 0; /* shared identity */
    for (int i = 0; i < N; ++i) if (i != slot && actual[i] == g) return 0;
    actual[slot] = g;
    return 1;
}
static int check(Frame *f) {
    reachable();
    for (int i = 0; i < N; ++i) actual[i] = 0;
    for (int r = 0; r < 3; ++r) if (!bind(roots[r], f->roots[r])) return 0;
    for (int pass = 0; pass < N; ++pass)
        for (int i = 0; i < N; ++i) if (actual[i])
            for (int e = 0; e < 2; ++e) if (!bind(edges[i][e], actual[i]->edge[e])) return 0;
    for (int i = 0; i < N; ++i) if (alive[i] != (actual[i] != 0)) return 0;
    return 1;
}
int lo_entry(void) {
    lo_runtime_init();
    Frame frame = {0, 3, {0, 0, 0}};
    lo_push_frame(&frame);
    for (int r = 0; r < 3; ++r) roots[r] = -1;
    /* Start with a nontrivial cycle and a shared root before random mutations. */
    for (int i = 0; i < 16; ++i) {
        Graph *fresh = (Graph *)lo_alloc(&klass);
        fresh->id = ids[i] = ++serial;
        fresh->payload = serial ^ 0xabc123u;
        edges[i][0] = roots[0]; edges[i][1] = -1;
        fresh->edge[0] = frame.roots[0];
        roots[0] = i; frame.roots[0] = (Object *)fresh;
        if (i == 0) { roots[1] = 0; frame.roots[1] = (Object *)fresh; }
    }
    edges[0][1] = 15;
    lo_gc_write_barrier(frame.roots[1], offsets[1], frame.roots[0]);
    roots[2] = 15; frame.roots[2] = frame.roots[0];
    for (int step = 0; step < STEPS; ++step) {
        REQUIRE(check(&frame));
        int r = random32() % 3, op = random32() % 4;
        if (op == 0) {
            int slot = 0; while (slot < N && alive[slot]) ++slot;
            if (slot < N) {
                /* Allocation may move every old object. Store the return value
                 * immediately; never use an actual[] pointer across allocation. */
                Graph *fresh = (Graph *)lo_alloc(&klass);
                fresh->id = ids[slot] = ++serial;
                fresh->payload = serial ^ 0xabc123u;
                edges[slot][0] = edges[slot][1] = -1;
                roots[r] = slot; frame.roots[r] = (Object *)fresh;
            }
        } else if (op == 1) {
            int a = random32() % N, b = random32() % (N + 1), e = random32() % 2;
            if (alive[a] && (b == N || alive[b])) {
                edges[a][e] = b == N ? -1 : b;
                lo_gc_write_barrier((Object *)actual[a], offsets[e], b == N ? 0 : (Object *)actual[b]);
            }
        } else if (op == 2) {
            int b = random32() % (N + 1);
            if (b == N || alive[b]) {
                roots[r] = b == N ? -1 : b;
                frame.roots[r] = b == N ? 0 : (Object *)actual[b];
            }
        } else {
            unsigned char bytes[137];
            for (unsigned i = 0; i < sizeof(bytes); ++i) bytes[i] = (unsigned char)(step + i);
            (void)lo_string_new(bytes, random32() % sizeof(bytes));
        }
        REQUIRE(check(&frame));
        lo_gc_collect();
        REQUIRE(check(&frame));
    }
    for (int r = 0; r < 3; ++r) { roots[r] = -1; frame.roots[r] = 0; }
    lo_gc_collect(); REQUIRE(check(&frame));
    lo_pop_frame(); lo_runtime_shutdown(); return 0;
}
#ifndef __wasm__
int main(void) { return lo_entry(); }
#endif
