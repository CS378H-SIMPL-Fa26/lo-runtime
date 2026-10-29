#ifndef GC_TEST_ABI_H
#define GC_TEST_ABI_H
#include <stddef.h>
#include <stdint.h>

typedef struct Class Class;
typedef struct Object { const Class *class; uint32_t gc_bits, flags; } Object;
struct Class {
    const char *name;
    uint32_t name_len;
    const Class *parent;
    uint32_t instance_size;
    const uint32_t *pointer_offsets;
    uint32_t pointer_count, vtable_size;
    const void *vtable;
};
typedef struct Node { Object header; Object *next; } Node;
typedef struct Frame { struct Frame *parent; uint32_t count; Object *roots[3]; } Frame;
typedef struct String { Object header; uint32_t length; unsigned char data[]; } String;
extern void lo_runtime_init(void), lo_runtime_shutdown(void), lo_gc_collect(void);
extern Object *lo_alloc(const Class *);
extern void lo_push_frame(Frame *), lo_pop_frame(void);
extern void lo_gc_write_barrier(Object *, uint32_t, Object *);
extern void lo_gc_register_static(Object *);
extern Object *lo_string_new(const unsigned char *, uint32_t);
extern Object *lo_string_concat(Object *, Object *);
extern Object *lo_string_reverse(Object *);
extern int32_t lo_string_compare(Object *, Object *);
extern const String LO_EMPTY_STRING;

#endif
