//! GC operations (`runtime-abi.md` §3.4) — the WS-2 reference **Cheney
//! two-space copying collector**.
//!
//! `lo_gc_write_barrier` is **provided** as a bare store — the correct
//! non-generational barrier (Cheney's semispace, mark-compact) out of the box,
//! and left untouched here (runbook WS-2 §2.6). `lo_gc_collect` performs a flip,
//! a root scan, and a Cheney scan over the to-space (runbook WS-2 §2.4); the
//! allocator's OOM path (`crate::alloc`) drives it.
//!
//! ## Forwarding-flag bit (decision (C))
//!
//! Bit 0 of `gc_bits` (`GC_FORWARDED_BIT == 0b1`) is the **forwarding flag**: set
//! on a from-space object once it has been evacuated, at which point its
//! `class_descriptor` slot holds the new to-space address instead of a descriptor
//! pointer (the tombstone). The bit is collector-private — no two menu collectors
//! run on one heap — so it needs no ABI pin. All three skeletons (Rust / Zig /
//! C++) use this same bit so cross-skeleton header encoding stays byte-identical.

use crate::layout::layout;
use crate::object::{ClassDescriptor, Object};
use crate::shadow_stack::roots;

/// Forwarding-flag bit in `gc_bits` (decision (C)). See module docs.
pub(crate) const GC_FORWARDED_BIT: u32 = 0b1;

/// Pointer-store write barrier. Non-generational implementation: a bare store of
/// `value` into the pointer field at `obj + field_offset` (`runtime-abi.md` §3.4).
/// Codegen emits a call to this at *every* pointer store, which makes the choice
/// of collector a runtime-only decision. Correct as-is for Cheney; left untouched
/// (runbook WS-2 §2.6).
///
/// # Safety
/// `obj` must point at a valid object with a pointer-typed field at
/// `field_offset`; `value` must be a valid object pointer or null.
#[no_mangle]
pub unsafe extern "C" fn lo_gc_write_barrier(
    obj: *mut Object,
    field_offset: u32,
    value: *mut Object,
) {
    let slot = (obj as *mut u8).add(field_offset as usize) as *mut *mut Object;
    *slot = value;
}

/// Force a collection (`runtime-abi.md` §3.4): a Cheney flip-and-scan cycle.
/// Reachable objects are copied to the inactive semispace with their interior
/// pointers and the shadow-stack roots rewritten to the new addresses;
/// unreachable objects are reclaimed when the old space is abandoned.
#[no_mangle]
pub extern "C" fn lo_gc_collect() {
    // SAFETY: single-threaded runtime; the heap is initialized (a collect before
    // `heap_init` walks an empty root set and flips empty semispaces — a no-op).
    unsafe { collect() }
}

/// Evacuate `p` from the from-space `[from_base, from_limit)` into the to-space,
/// bumping `free` (runbook WS-2 §2.3). Returns `p`'s new address. Null and
/// non-from-space (immortal / `.rodata`, e.g. `LO_EMPTY_STRING`) pointers are
/// returned untouched — the range-check is what keeps immortal objects safe with
/// no special case. An already-evacuated object (forwarding bit set) returns the
/// new address stored in its clobbered `class_descriptor` slot.
///
/// Order matters: the size is read and the bytes copied **before** the original's
/// `class_descriptor` is overwritten, because both reads need the intact
/// descriptor. The copy carries the intact descriptor into to-space with the
/// forwarding flag still clear; only the original becomes a tombstone.
///
/// # Safety
/// `p` is null or a valid object pointer. `free` points at enough room in to-space
/// for the largest live object. Single-threaded.
unsafe fn forward(
    p: *mut Object,
    from_base: *mut u8,
    from_limit: *mut u8,
    free: &mut *mut u8,
) -> *mut Object {
    if p.is_null() {
        return p;
    }
    let addr = p as *mut u8;
    if addr < from_base || addr >= from_limit {
        return p; // immortal / non-heap: leave untouched
    }
    if (*p).gc_bits & GC_FORWARDED_BIT != 0 {
        // Already moved; the class_descriptor slot holds the new address.
        return (*p).class_descriptor as *mut Object;
    }
    let n = layout(p).size.bytes();
    let dst = *free;
    core::ptr::copy_nonoverlapping(addr, dst, n);
    // Install the tombstone in the *original* — after the copy, so the to-space
    // copy keeps the intact descriptor and a clear forwarding flag.
    (*p).class_descriptor = dst as *const ClassDescriptor;
    (*p).gc_bits |= GC_FORWARDED_BIT;
    *free = dst.add(n);
    dst as *mut Object
}

/// One Cheney collection cycle (runbook WS-2 §2.4).
///
/// # Safety
/// Single-threaded; the allocator statics describe a valid two-semispace heap (or
/// a null/uninitialized heap, in which case both spaces are empty and this is a
/// no-op).
unsafe fn collect() {
    let (from_base, from_limit) = crate::alloc::from_space();
    let to_base = crate::alloc::to_space_base();
    let mut free = to_base;
    let mut scan = to_base;

    // 1–2. Scan the root set.
    for slot in roots() {
        slot.set(forward(slot.get(), from_base, from_limit, &mut free));
    }

    // 3. Cheney loop: scan copied objects in to-space, forwarding each pointer
    // field, until `scan` catches `free`. Strings have no pointer fields, so they
    // only advance `scan`, by their variable size.
    while scan < free {
        let obj = scan as *mut Object;
        let layout = layout(obj);
        for slot in layout.pointer_slots(obj) {
            slot.set(forward(slot.get(), from_base, from_limit, &mut free));
        }
        scan = scan.add(layout.size.bytes());
    }

    // 4. Commit: swap the semispace roles; the old from-space is now free.
    crate::alloc::flip(free);

    // Debug instrumentation (off unless `LO_GC_POISON` is set): scribble the
    // just-abandoned from-space with a poison byte. Survivors were already copied
    // out to the new from-space, so this only clobbers garbage and tombstones —
    // but it turns any *stale* pointer (one the collector did not relocate because
    // it was not a registered root) into a deterministic read of `0xEFEFEFEF…`
    // instead of its possibly-still-intact old bytes. That makes a missing
    // GC root a hard failure rather than a flaky one (see the WS-11 GC-rooting
    // stress test). Not part of the ABI; zero cost when disabled.
    if poison_enabled() {
        let len = (from_limit as usize) - (from_base as usize);
        core::ptr::write_bytes(from_base, 0xEF, len);
    }
}

/// Whether `LO_GC_POISON` requests poisoning the abandoned from-space after each
/// collection (debug aid; see `collect`). Read once and cached. Always false on
/// freestanding WASM (no environment).
fn poison_enabled() -> bool {
    use std::sync::OnceLock;
    static POISON: OnceLock<bool> = OnceLock::new();
    *POISON.get_or_init(|| {
        std::env::var("LO_GC_POISON")
            .map(|v| !v.is_empty() && v != "0")
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    //! Cheney collector tests (runbook WS-2 §3, step A.4). Run in-crate so they
    //! can reach the internal string allocator and heap-occupancy hook.
    //!
    //! The runtime is process-global single-threaded state, so these tests
    //! serialize through a mutex and bracket each body with init/shutdown. The
    //! `LO_HEAP_SIZE` override (read by `heap_init`) is set inside the lock so the
    //! allocation-pressure test cannot race another test's init.

    use std::sync::Mutex;

    use crate::alloc::{bump_alloc_string, heap_used, lo_alloc};
    use crate::descriptors::{LO_EMPTY_STRING, LO_STRING_CLASS};
    use crate::object::{string_data_offset, ClassDescriptor, Object, ShadowFrame, StringObject};
    use crate::shadow_stack::{lo_pop_frame, lo_push_frame};
    use crate::{lo_gc_collect, lo_runtime_init, lo_runtime_shutdown};

    static LOCK: Mutex<()> = Mutex::new(());

    // A test class with a single pointer field at offset 16 (right after the
    // 16-byte header): a linked-list "Node". instance_size 24, one pointer field.
    static NODE_PTR_OFFSETS: [u32; 1] = [16];
    static NODE_CLASS: ClassDescriptor = ClassDescriptor {
        name: c"Node".as_ptr() as *const u8,
        name_len: 4,
        parent: core::ptr::null(),
        instance_size: 24,
        pointer_offsets: &NODE_PTR_OFFSETS as *const u32,
        pointer_count: 1,
        vtable_size: 0,
        vtable: core::ptr::null(),
    };

    /// A shadow-stack frame with `N` inline roots, byte-compatible with the ABI's
    /// `ShadowFrame { parent, num_roots, roots: [*mut Object; N] }`.
    #[repr(C)]
    struct TestFrame<const N: usize> {
        parent: *mut ShadowFrame,
        num_roots: u32,
        roots: [*mut Object; N],
    }

    impl<const N: usize> TestFrame<N> {
        fn new() -> Self {
            TestFrame {
                parent: core::ptr::null_mut(),
                num_roots: N as u32,
                roots: [core::ptr::null_mut(); N],
            }
        }
        fn as_frame(&mut self) -> *mut ShadowFrame {
            self as *mut TestFrame<N> as *mut ShadowFrame
        }
    }

    /// Run `f` with the runtime up (optionally with a small heap to force GC),
    /// serialized and bracketed by init/shutdown.
    fn with_runtime<T>(heap_size: Option<&str>, f: impl FnOnce() -> T) -> T {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        match heap_size {
            Some(s) => std::env::set_var("LO_HEAP_SIZE", s),
            None => std::env::remove_var("LO_HEAP_SIZE"),
        }
        lo_runtime_init();
        let result = f();
        lo_runtime_shutdown();
        std::env::remove_var("LO_HEAP_SIZE");
        result
    }

    unsafe fn set_field(obj: *mut Object, off: usize, val: *mut Object) {
        *((obj as *mut u8).add(off) as *mut *mut Object) = val;
    }
    unsafe fn get_field(obj: *mut Object, off: usize) -> *mut Object {
        *((obj as *mut u8).add(off) as *mut *mut Object)
    }

    // (i) A known live graph survives: reachable objects are preserved with
    // updated addresses, and both the root slot and the interior pointer field
    // are rewritten to the new locations.
    #[test]
    fn live_graph_preserved_and_pointers_rewritten() {
        with_runtime(None, || unsafe {
            let mut frame = TestFrame::<1>::new();
            lo_push_frame(frame.as_frame());

            let a = lo_alloc(&NODE_CLASS);
            let b = lo_alloc(&NODE_CLASS);
            set_field(a, 16, b); // a.next = b
            frame.roots[0] = a; // root = a

            lo_gc_collect();

            let a2 = frame.roots[0];
            assert_ne!(a2, a, "root must be rewritten to the moved object");
            assert_eq!((*a2).class_descriptor, &NODE_CLASS as *const _);

            let b2 = get_field(a2, 16);
            assert!(!b2.is_null(), "interior pointer must survive");
            assert_ne!(b2, b, "interior pointer must be rewritten to b's new addr");
            assert_eq!((*b2).class_descriptor, &NODE_CLASS as *const _);
            assert!(get_field(b2, 16).is_null(), "b.next was null");

            lo_pop_frame();
        });
    }

    // (ii) An unreachable object is reclaimed: heap occupancy drops by its size.
    #[test]
    fn unreachable_object_reclaimed() {
        with_runtime(None, || unsafe {
            let mut frame = TestFrame::<1>::new();
            lo_push_frame(frame.as_frame());

            let live = lo_alloc(&NODE_CLASS);
            frame.roots[0] = live;
            let _dead = lo_alloc(&NODE_CLASS); // not rooted -> unreachable

            let before = heap_used();
            assert_eq!(before, 48, "two 24-byte nodes occupy 48 bytes");

            lo_gc_collect();

            let after = heap_used();
            assert_eq!(after, 24, "only the live node survives");
            assert!(after < before);

            // The survivor's root was rewritten to its moved location and is
            // still a valid Node. (Also makes the root store observably-live to
            // the compiler, which cannot see the collector read it via the
            // registered shadow-stack pointer.)
            let live2 = frame.roots[0];
            assert_eq!((*live2).class_descriptor, &NODE_CLASS as *const _);

            lo_pop_frame();
        });
    }

    // (iii) A variable-size String survives a collection intact (bytes round-trip).
    #[test]
    fn string_survives_collection() {
        with_runtime(None, || unsafe {
            let mut frame = TestFrame::<1>::new();
            lo_push_frame(frame.as_frame());

            let bytes: &[u8] = b"hello, Cheney";
            let s = bump_alloc_string(bytes.len() as u32);
            let data = (s as *mut u8).add(string_data_offset());
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), data, bytes.len());
            frame.roots[0] = s;

            lo_gc_collect();

            let s2 = frame.roots[0];
            assert_ne!(s2, s, "string must move to to-space");
            assert_eq!((*s2).class_descriptor, &LO_STRING_CLASS as *const _);
            let so = s2 as *const StringObject;
            assert_eq!((*so).length, bytes.len() as u32);
            let data2 = (s2 as *const u8).add(string_data_offset());
            let round = core::slice::from_raw_parts(data2, bytes.len());
            assert_eq!(
                round, bytes,
                "string bytes must round-trip through the copy"
            );

            lo_pop_frame();
        });
    }

    // (iv) LO_EMPTY_STRING is immortal: a moved object's field that references it
    // still points at the same `.rodata` address after collection (forward's
    // range-check leaves non-from-space pointers untouched — no special case).
    #[test]
    fn empty_string_not_moved() {
        with_runtime(None, || unsafe {
            let mut frame = TestFrame::<1>::new();
            lo_push_frame(frame.as_frame());

            let holder = lo_alloc(&NODE_CLASS);
            let empty = &LO_EMPTY_STRING as *const StringObject as *mut Object;
            set_field(holder, 16, empty); // holder.next = &LO_EMPTY_STRING
            frame.roots[0] = holder;

            lo_gc_collect();

            let holder2 = frame.roots[0];
            assert_ne!(holder2, holder, "holder itself moves");
            let field = get_field(holder2, 16);
            assert_eq!(
                field, empty,
                "immortal LO_EMPTY_STRING must not be relocated"
            );

            lo_pop_frame();
        });
    }

    // (v) Allocation pressure past one semispace triggers GC through lo_alloc's
    // OOM path and continues; only a genuine live-set overflow would abort.
    #[test]
    fn allocation_pressure_triggers_gc() {
        // Total 4 KiB -> 2 KiB per semispace (~85 nodes). 600 allocations of dead
        // nodes force many collections; the single rooted node survives them all.
        with_runtime(Some("4096"), || unsafe {
            let mut frame = TestFrame::<1>::new();
            lo_push_frame(frame.as_frame());

            let live = lo_alloc(&NODE_CLASS);
            frame.roots[0] = live;

            for _ in 0..600 {
                // Not rooted: each becomes garbage the next collection reclaims.
                let _dead = lo_alloc(&NODE_CLASS);
            }

            // Reaching here means GC repeatedly reclaimed and allocation continued
            // (a missing GC trigger would have aborted with exit 137, killing the
            // test process). The rooted node survived every collection.
            let live2 = frame.roots[0];
            assert_eq!((*live2).class_descriptor, &NODE_CLASS as *const _);

            lo_pop_frame();
        });
    }
}
