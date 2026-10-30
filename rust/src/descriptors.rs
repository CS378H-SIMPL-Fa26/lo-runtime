//! The runtime's special exported statics (`runtime-abi.md` §2.3): the built-in
//! class descriptors and the canonical empty-string singleton. Codegen
//! references these by symbol.

use crate::object::{ClassDescriptor, Object, StringObject};

/// Descriptor for the well-known `String` class.
#[no_mangle]
pub static LO_STRING_CLASS: ClassDescriptor = ClassDescriptor {
    name: c"String".as_ptr() as *const u8,
    name_len: 6,
    parent: core::ptr::null(),
    instance_size: core::mem::size_of::<StringObject>() as u32,
    pointer_offsets: core::ptr::null(),
    pointer_count: 0,
    vtable_size: 0,
    vtable: core::ptr::null(),
};

/// Descriptor for a boxed `int` (used only by LO-5 features that need uniform
/// pointer-typed values). Header plus a 4-byte boxed slot.
#[no_mangle]
pub static LO_INT_BOX_CLASS: ClassDescriptor = ClassDescriptor {
    name: c"int".as_ptr() as *const u8,
    name_len: 3,
    parent: core::ptr::null(),
    instance_size: (core::mem::size_of::<Object>() + 4) as u32,
    pointer_offsets: core::ptr::null(),
    pointer_count: 0,
    vtable_size: 0,
    vtable: core::ptr::null(),
};

/// Descriptor for a boxed `bool`. Header plus a 4-byte boxed slot (kept the same
/// width as the int box for a uniform boxed-primitive layout).
#[no_mangle]
pub static LO_BOOL_BOX_CLASS: ClassDescriptor = ClassDescriptor {
    name: c"bool".as_ptr() as *const u8,
    name_len: 4,
    parent: core::ptr::null(),
    instance_size: (core::mem::size_of::<Object>() + 4) as u32,
    pointer_offsets: core::ptr::null(),
    pointer_count: 0,
    vtable_size: 0,
    vtable: core::ptr::null(),
};

/// The canonical length-0 `StringObject` (`runtime-abi.md` §2.3, runbook WS-2
/// §2.5). Defined as a **read-only static object in `.rodata`** — not
/// heap-allocated — so its address is a link-time constant that lies outside the
/// managed heap by construction. The symbol denotes the object itself; codegen
/// references its address directly (no load) to initialize String-typed fields to
/// their LO default (the empty string).
///
/// This replaces the WS-1 init-time allocation: `lo_runtime_init` no longer
/// allocates the empty string. Because it is never *in* the heap, the
/// collector's range check (`Region::locate`) leaves it untouched with no special
/// case (runbook WS-2 §2.5), and being read-only storage, a buggy collector that
/// forgot to range-guard before writing its header would fault loudly rather than
/// corrupt a shared object.
///
/// A zero-length string has no inline tail, so the whole object is a fixed
/// compile-time constant. `StringObject` carries a raw pointer (in its `Object`
/// header), so it is not `Sync` by default — but this instance is immutable
/// read-only data on the single runtime thread, so the `unsafe impl Sync` below
/// is sound, exactly as for the class descriptors above.
#[no_mangle]
pub static LO_EMPTY_STRING: StringObject = StringObject {
    header: Object {
        class_descriptor: &LO_STRING_CLASS as *const ClassDescriptor,
        gc_bits: 0,
        flags: 0,
    },
    length: 0,
    data: [],
};

// SAFETY: `StringObject` carries a raw pointer in its header (so it is not `Sync`
// by default), but `LO_EMPTY_STRING` is immutable read-only data, generated once
// and never mutated, and the runtime is single-threaded (`runtime-abi.md` §1). It
// is therefore sound to place it in a `static`, which requires `Sync`.
unsafe impl Sync for StringObject {}
