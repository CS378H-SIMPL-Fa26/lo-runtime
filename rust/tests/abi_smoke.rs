//! Language-native ABI smoke tests for the provided entry points, driven
//! directly through the C-ABI surface (`runtime-abi.md` §4.4).
//!
//! The runtime is process-global single-threaded state, so these tests serialize
//! through a mutex and bracket each body with init/shutdown. Stubbed entry points
//! are not tested here — calling them panics by design.

use std::sync::Mutex;

use lo_runtime::{
    current_frame, lo_alloc, lo_pop_frame, lo_push_frame, lo_runtime_init, lo_runtime_shutdown,
    Object, ShadowFrame, StringObject, LO_EMPTY_STRING, LO_INT_BOX_CLASS, LO_STRING_CLASS,
};

static LOCK: Mutex<()> = Mutex::new(());

/// Run `f` with the runtime initialized, serialized against other tests, and
/// shut down afterward. Tolerates a poisoned lock from a previously-panicking
/// test.
fn with_runtime<T>(f: impl FnOnce() -> T) -> T {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    lo_runtime_init();
    let result = f();
    lo_runtime_shutdown();
    result
}

#[test]
fn init_shutdown_clean() {
    with_runtime(|| {});
    // A second cycle, plus a shutdown with no prior init, must not panic.
    with_runtime(|| {});
    lo_runtime_shutdown();
}

#[test]
fn alloc_returns_zeroed() {
    with_runtime(|| unsafe {
        let obj = lo_alloc(&LO_INT_BOX_CLASS);
        assert!(!obj.is_null());

        // Header is stamped correctly.
        assert_eq!((*obj).class_descriptor, &LO_INT_BOX_CLASS as *const _);
        assert_eq!((*obj).gc_bits, 0);
        assert_eq!((*obj).flags, 0);

        // Every byte after the 16-byte header, up to the rounded allocation
        // size, is zero. instance_size is 20 -> rounded to 24.
        let base = obj as *const u8;
        for off in core::mem::size_of::<Object>()..24 {
            assert_eq!(*base.add(off), 0, "byte at offset {off} not zeroed");
        }
    });
}

#[test]
fn shadow_stack_push_pop() {
    with_runtime(|| {
        assert!(current_frame().is_null());

        let mut f1 = ShadowFrame {
            parent: core::ptr::null_mut(),
            num_roots: 0,
            roots: [],
        };
        let mut f2 = ShadowFrame {
            parent: core::ptr::null_mut(),
            num_roots: 0,
            roots: [],
        };
        let f1_addr = &f1 as *const ShadowFrame as usize;
        let f2_addr = &f2 as *const ShadowFrame as usize;

        unsafe { lo_push_frame(&mut f1) };
        assert_eq!(current_frame() as usize, f1_addr);

        unsafe { lo_push_frame(&mut f2) };
        assert_eq!(current_frame() as usize, f2_addr);
        assert_eq!(f2.parent as usize, f1_addr);

        lo_pop_frame();
        assert_eq!(current_frame() as usize, f1_addr);

        lo_pop_frame();
        assert!(current_frame().is_null());
    });
}

#[test]
fn empty_string_is_rodata_static() {
    // LO_EMPTY_STRING is now a `.rodata` static *object* (decision (A), ABI §2.3),
    // not an init-time heap allocation: the symbol denotes the object itself, so
    // it is a valid length-0 String independent of any init/shutdown cycle.
    let so: &StringObject = &LO_EMPTY_STRING;
    assert_eq!(
        so.header.class_descriptor, &LO_STRING_CLASS as *const _,
        "LO_EMPTY_STRING must be backed by LO_STRING_CLASS"
    );
    assert_eq!(so.length, 0, "LO_EMPTY_STRING must be length 0");
}

#[test]
fn alloc_advances_bump_pointer() {
    with_runtime(|| unsafe {
        #[repr(C)]
        struct RootFrame {
            parent: *mut ShadowFrame,
            num_roots: u32,
            roots: [*mut Object; 1],
        }
        let mut frame = RootFrame {
            parent: core::ptr::null_mut(),
            num_roots: 1,
            roots: [core::ptr::null_mut()],
        };
        lo_push_frame((&mut frame as *mut RootFrame).cast());
        frame.roots[0] = lo_alloc(&LO_INT_BOX_CLASS);
        let b = lo_alloc(&LO_INT_BOX_CLASS);
        let a = frame.roots[0];
        assert_ne!(a, b, "distinct live allocations must not alias");
        // instance_size 20 rounds to 24; the second object starts 24 bytes on.
        assert_eq!(b as usize - a as usize, 24);
        lo_pop_frame();
    });
}
