//! GC operations (`runtime-abi.md` §3.4): a stop-the-world, in-place,
//! order-preserving **sliding mark-compact** collector.
//!
//! `lo_gc_write_barrier` is a bare store, the correct barrier for a
//! non-generational collector. `lo_gc_collect` runs one cycle; the allocator's
//! OOM path (`crate::alloc`) drives it.
//!
//! A cycle has four phases, each a type only the previous phase can produce:
//!
//! 1. [`Marking`] clears the [`MarkBitmap`] and traces from the shadow-stack
//!    roots, setting the bit of every granule of every reachable object.
//! 2. [`Marked::plan`] builds the [`OffsetTable`] in one pass over the bitmap.
//! 3. [`Plan::fix_roots`] forwards every root.
//! 4. [`Plan::compact`] slides each live object, in address order, down to its
//!    forwarded address and forwards its reference fields.
//!
//! [`Forwarding::forward`] reads only the two tables, never the heap, so it gives
//! the same answer before, during and after objects move. A [`Plan`] holds the
//! tables by shared reference and the heap region by unique reference, so the
//! tables cannot change while objects move. The collector uses no header bits:
//! `gc_bits` stays 0.

mod bitmap;
mod offsets;

use core::ptr;

pub(crate) use bitmap::MarkBitmap;
pub(crate) use offsets::OffsetTable;

use crate::alloc::{with_heap, Heap, Region};
use crate::layout::{layout, Granule};
use crate::object::Object;
use crate::shadow_stack::roots;

/// Pointer-store write barrier. Non-generational implementation: a bare store of
/// `value` into the pointer field at `obj + field_offset` (`runtime-abi.md` §3.4).
/// Codegen emits a call to this at *every* pointer store, which makes the choice
/// of collector a runtime-only decision.
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

/// Force a collection (`runtime-abi.md` §3.4). Reachable objects slide to the
/// bottom of the heap, keeping their order, with the shadow-stack roots and their
/// reference fields rewritten to the new addresses; the space above them is free.
/// A no-op before `lo_runtime_init`.
#[no_mangle]
pub extern "C" fn lo_gc_collect() {
    with_heap(|heap| {
        if let Some(heap) = heap {
            collect(heap);
        }
    });
}

/// One collection cycle.
pub(crate) fn collect(heap: &mut Heap) {
    let old_top = heap.top;
    let plan = Marking::begin(heap).trace().plan();
    plan.fix_roots();
    let new_top = plan.compact();
    heap.top = new_top;

    // Debug instrumentation (off unless `LO_GC_POISON` is set): scribble the
    // vacated tail `[new_top, old_top)` with a poison byte. Nothing live is there
    // any more, so a *stale* pointer into it (one the collector did not relocate
    // because it was not a registered root) reads a deterministic `0xEFEFEFEF…`
    // instead of possibly-still-intact old bytes. Not part of the ABI; zero cost
    // when disabled.
    if poison_enabled() {
        let tail = heap.region.addr(new_top) as *mut u8;
        // SAFETY: `[new_top, old_top)` lies inside the region and holds no object.
        unsafe { ptr::write_bytes(tail, 0xEF, old_top.bytes() - new_top.bytes()) };
    }
}

/// Phase 1: tracing from the roots into a cleared bitmap.
#[must_use]
struct Marking<'h> {
    heap: &'h mut Heap,
}

impl<'h> Marking<'h> {
    fn begin(heap: &'h mut Heap) -> Marking<'h> {
        heap.bitmap.clear_below(heap.top);
        Marking { heap }
    }

    /// Mark every object reachable from the roots.
    fn trace(mut self) -> Marked<'h> {
        for slot in roots() {
            self.discover(slot.get());
        }
        while let Some(g) = self.heap.mark_stack.pop() {
            let obj = self.heap.region.addr(g);
            // SAFETY: `g` was located in `[0, top)`, so it starts an object whose
            // header is intact (nothing moves while marking).
            let layout = unsafe { layout(obj) };
            for slot in unsafe { layout.pointer_slots(obj) } {
                self.discover(slot.get());
            }
        }
        Marked { heap: self.heap }
    }

    /// Mark the object `p` refers to, if it is in the heap and not yet marked,
    /// and queue it for scanning. Marking on push keeps each object on the stack
    /// at most once.
    fn discover(&mut self, p: *mut Object) {
        let heap = &mut *self.heap;
        let Some(g) = heap.region.locate(p, heap.top) else {
            return;
        };
        if heap.bitmap.is_marked(g) {
            return;
        }
        // SAFETY: as in `trace`.
        let size = unsafe { layout(heap.region.addr(g)) }.size;
        heap.bitmap.mark(g, size);
        heap.mark_stack.push(g);
    }
}

/// Phase 2: the bitmap holds exactly the live granules.
#[must_use]
struct Marked<'h> {
    heap: &'h mut Heap,
}

impl<'h> Marked<'h> {
    /// Build the offset table and freeze both tables for forwarding.
    fn plan(self) -> Plan<'h> {
        let Heap {
            region,
            top,
            bitmap,
            offsets,
            ..
        } = self.heap;
        let new_top = offsets.build(bitmap, *top);
        Plan {
            region,
            fwd: Forwarding { bitmap, offsets },
            top: *top,
            new_top,
        }
    }
}

/// Where each live granule lands once the heap is compacted.
#[derive(Clone, Copy)]
struct Forwarding<'h> {
    bitmap: &'h MarkBitmap,
    offsets: &'h OffsetTable,
}

impl Forwarding<'_> {
    /// The live granules before `g`: those in earlier blocks plus those below it
    /// in its own block.
    fn forward(self, g: Granule) -> Granule {
        Granule::new(self.offsets.get(g.block()) + self.bitmap.live_before(g))
    }
}

/// Phases 3 and 4: the tables are final; roots and objects can move.
#[must_use]
struct Plan<'h> {
    region: &'h mut Region,
    fwd: Forwarding<'h>,
    /// The top before compaction: the end of the scan, and the range references
    /// are located in until they are forwarded.
    top: Granule,
    new_top: Granule,
}

impl Plan<'_> {
    /// The post-compaction address of `p`; null and non-heap pointers unchanged.
    fn relocate(&self, p: *mut Object) -> *mut Object {
        match self.region.locate(p, self.top) {
            Some(g) => self.region.addr(self.fwd.forward(g)),
            None => p,
        }
    }

    fn fix_roots(&self) {
        for slot in roots() {
            slot.set(self.relocate(slot.get()));
        }
    }

    /// Slide every live object down to its forwarded address and forward its
    /// reference fields, returning the new top. Objects are visited in address
    /// order and every destination is at or below its source, so a move only
    /// ever overwrites objects already visited or garbage.
    fn compact(self) -> Granule {
        let mut next = self.fwd.bitmap.next_set(Granule::ZERO, self.top);
        while let Some(g) = next {
            let src = self.region.addr(g);
            // Read before moving: an overlapping copy can overwrite the header
            // (and a string's length). The descriptor is static, so the layout
            // stays valid for the copy.
            // SAFETY: `g` starts a live object that has not moved yet.
            let layout = unsafe { layout(src) };
            let dst = self.region.addr(self.fwd.forward(g));
            if dst != src {
                // SAFETY: both ranges lie in the region; `dst < src`, and
                // `ptr::copy` handles the overlap.
                unsafe { ptr::copy(src as *const u8, dst as *mut u8, layout.size.bytes()) };
            }
            // SAFETY: the object now lives at `dst` and stays there.
            for slot in unsafe { layout.pointer_slots(dst) } {
                slot.set(self.relocate(slot.get()));
            }
            next = self.fwd.bitmap.next_set(g + layout.size, self.top);
        }
        self.new_top
    }
}

/// Whether `LO_GC_POISON` requests poisoning the vacated tail after each
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
    //! Collector tests. Run in-crate so they can reach the internal string
    //! allocator, the heap-occupancy hook and the forwarding function.
    //!
    //! The runtime is process-global single-threaded state, so these tests
    //! serialize through a mutex and bracket each body with init/shutdown. The
    //! `LO_HEAP_SIZE` override (read by `heap_init`) is set inside the lock so the
    //! allocation-pressure test cannot race another test's init.

    use std::sync::Mutex;

    use super::{Forwarding, MarkBitmap, OffsetTable};
    use crate::alloc::{bump_alloc_string, heap_used, lo_alloc};
    use crate::descriptors::{LO_EMPTY_STRING, LO_STRING_CLASS};
    use crate::layout::{Granule, Granules};
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

    // Forwarding counts the live granules before an object, across blocks.
    #[test]
    fn forwarding_counts_live_granules_before() {
        let mut bitmap = MarkBitmap::new(2);
        for (start, size) in [(2, 3), (10, 2), (62, 4), (70, 1)] {
            bitmap.mark(Granule::new(start), Granules::new(size));
        }
        let mut offsets = OffsetTable::new(2);
        assert_eq!(offsets.build(&bitmap, Granule::new(71)), Granule::new(10));
        let fwd = Forwarding {
            bitmap: &bitmap,
            offsets: &offsets,
        };
        let landed = [2, 10, 62, 70].map(|g| fwd.forward(Granule::new(g)).index());
        assert_eq!(landed, [0, 3, 5, 9]);
    }

    // (i) A known live graph survives: reachable objects slide down over the
    // garbage below them, and both the root slot and the interior pointer field
    // are rewritten to the new locations.
    #[test]
    fn live_graph_preserved_and_pointers_rewritten() {
        with_runtime(None, || unsafe {
            let mut frame = TestFrame::<1>::new();
            lo_push_frame(frame.as_frame());

            let _dead = lo_alloc(&NODE_CLASS);
            let a = lo_alloc(&NODE_CLASS);
            let b = lo_alloc(&NODE_CLASS);
            set_field(a, 16, b); // a.next = b
            frame.roots[0] = a; // root = a

            lo_gc_collect();

            let a2 = frame.roots[0];
            assert!(a2 < a, "root must be rewritten to the slid object");
            assert_eq!((*a2).class_descriptor, &NODE_CLASS as *const _);

            let b2 = get_field(a2, 16);
            assert!(!b2.is_null(), "interior pointer must survive");
            assert!(b2 < b, "interior pointer must be rewritten to b's new addr");
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

            // The survivor's root still refers to a valid Node. (Also makes the root store observably-live to
            // the compiler, which cannot see the collector read it via the
            // registered shadow-stack pointer.)
            let live2 = frame.roots[0];
            assert_eq!((*live2).class_descriptor, &NODE_CLASS as *const _);

            lo_pop_frame();
        });
    }

    // (iii) A variable-size String slides intact (bytes round-trip).
    #[test]
    fn string_survives_collection() {
        with_runtime(None, || unsafe {
            let mut frame = TestFrame::<1>::new();
            lo_push_frame(frame.as_frame());

            let _dead = lo_alloc(&NODE_CLASS);
            let bytes: &[u8] = b"hello, compactor";
            let s = bump_alloc_string(bytes.len() as u32);
            let data = (s as *mut u8).add(string_data_offset());
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), data, bytes.len());
            frame.roots[0] = s;

            lo_gc_collect();

            let s2 = frame.roots[0];
            assert!(s2 < s, "string must slide over the dead node");
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

    // (iv) LO_EMPTY_STRING is immortal: an object's field that references it still
    // points at the same `.rodata` address after collection (the collector only
    // forwards references it locates in the heap — no special case).
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
            let field = get_field(holder2, 16);
            assert_eq!(
                field, empty,
                "immortal LO_EMPTY_STRING must not be relocated"
            );

            lo_pop_frame();
        });
    }

    // (v) Allocation pressure past the heap triggers GC through lo_alloc's OOM path
    // and continues; only a genuine live-set overflow would abort.
    #[test]
    fn allocation_pressure_triggers_gc() {
        // A 4 KiB heap holds ~170 nodes. 600 allocations of dead nodes force many
        // collections; the single rooted node survives them all.
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
