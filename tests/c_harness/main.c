/*
 * Cross-skeleton ABI link test (runtime-abi.md §4, runbook WS-1 Phase 4.4).
 *
 * This is a pure C program — it does NOT include any skeleton's headers. It
 * re-declares only the slice of the C ABI it uses and links against a given
 * skeleton's static library (Rust / Zig / C++). If it links and runs, that
 * skeleton genuinely honors the C ABI from C code — the actual promise the three
 * skeletons make. All three must produce identical output.
 *
 * It also re-declares the Object / ClassDescriptor layout from C and
 * _Static_assert's the sizes/offsets, giving a fourth (C-side) witness to the
 * byte layout the three skeletons' own compile-time checks already pin.
 */
#include <stddef.h>
#include <stdint.h>

/* --- ABI types, as a C consumer sees them (runtime-abi.md §2) ------------- */
typedef struct ClassDescriptor ClassDescriptor;

typedef struct Object {
  const ClassDescriptor *class_descriptor;
  uint32_t gc_bits;
  uint32_t flags;
} Object;

struct ClassDescriptor {
  const char *name;
  uint32_t name_len;
  const ClassDescriptor *parent;
  uint32_t instance_size;
  const uint32_t *pointer_offsets;
  uint32_t pointer_count;
  uint32_t vtable_size;
  const void *vtable;
};

typedef struct StringObject {
  Object header;
  uint32_t length;
  /* inline bytes follow at offsetof(StringObject, length) + sizeof(uint32_t) */
} StringObject;

/* 64-bit layout locks, matching the Rust const_/Zig comptime/C++ static_assert. */
#if UINTPTR_MAX == 0xFFFFFFFFFFFFFFFFu
_Static_assert(sizeof(Object) == 16, "Object must be 16 bytes on 64-bit");
_Static_assert(offsetof(StringObject, length) + sizeof(uint32_t) == 20,
               "string data must start at offset 20 on 64-bit");
#endif

/* --- The C-ABI surface this harness exercises (the provided entry points) - */
extern void lo_runtime_init(void);
extern void lo_runtime_shutdown(void);
extern Object *lo_alloc(const ClassDescriptor *cls);
/* The print family's trailing to_stderr selects the stream: 0 = stdout,
 * 1 = stderr (runtime-abi.md §3.7). This harness only writes stdout. */
extern void lo_print_int(int32_t n, int32_t to_stderr);
extern void lo_print_string(Object *s, int32_t to_stderr);
extern void lo_println(int32_t to_stderr);

/* Exported statics codegen references by symbol. LO_EMPTY_STRING is a `.rodata`
 * static *object* (runtime-abi.md §2.3): the symbol denotes the object itself,
 * not a pointer variable, so it is declared as a StringObject and referenced by
 * address. */
extern const ClassDescriptor LO_INT_BOX_CLASS;
extern const ClassDescriptor LO_STRING_CLASS;
extern const StringObject LO_EMPTY_STRING;

int main(void) {
  lo_runtime_init();

  /* Allocate an object through the bump allocator and confirm the header the
   * runtime stamped is the descriptor we passed. */
  Object *o = lo_alloc(&LO_INT_BOX_CLASS);
  if (o == NULL || o->class_descriptor != &LO_INT_BOX_CLASS) {
    return 2;
  }

  /* The empty-string static must be a valid length-0 String (no init needed —
   * it is a `.rodata` constant). */
  if (LO_EMPTY_STRING.header.class_descriptor != &LO_STRING_CLASS ||
      LO_EMPTY_STRING.length != 0) {
    return 3;
  }

  /* Output the canonical line the cross-skeleton check compares: "42\n". */
  lo_print_int(42, 0);
  lo_println(0);

  lo_runtime_shutdown();
  return 0;
}
