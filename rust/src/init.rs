//! Runtime lifecycle (`runtime-abi.md` §3.6).

/// Initialize the runtime. Must be called before any other entry point: brings
/// up the heap and the collector's side tables, and resets the shadow-stack
/// head. Codegen emits a call at the top of `main` (native) or in the WASM
/// `start` function.
///
/// `LO_EMPTY_STRING` is a `.rodata` static and needs no initialization
/// (`runtime-abi.md` §2.3, §3.6; runbook WS-2 §2.5) — the WS-1 init-time
/// allocation is gone.
#[no_mangle]
pub extern "C" fn lo_runtime_init() {
    crate::alloc::heap_init();
    crate::shadow_stack::reset();
}

/// Tear down the runtime. Optional on native (the OS reclaims memory); useful in
/// tests to validate no-leak invariants. Safe to call without a prior init and
/// safe to call more than once.
#[no_mangle]
pub extern "C" fn lo_runtime_shutdown() {
    crate::shadow_stack::reset();
    crate::alloc::heap_shutdown();
}
