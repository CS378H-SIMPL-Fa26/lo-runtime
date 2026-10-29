//! The managed heap behind `lo_alloc` (`runtime-abi.md` §3.1): one contiguous
//! region with a bump pointer, compacted in place by the collector
//! ([`crate::gc`]).
//!
//! Granules `[0, top)` hold objects back to back with no gaps; allocation bumps
//! `top`. When a request does not fit, `lo_alloc` collects and retries once; only
//! a genuine live-set overflow aborts via the OOM contract (exit 137 native /
//! `unreachable` WASM).
//!
//! The heap is a 16 MiB default, overridable at `lo_runtime_init` via the
//! `LO_HEAP_SIZE` environment variable, and all of it is usable: the collector's
//! side tables are allocated alongside it. Setting `LO_HEAP_SIZE` small lets the
//! allocation-pressure tests force a collection deterministically.
//!
//! The heap is one global `Option<Heap>` — `None` before init and after
//! shutdown — reached only through [`with_heap`] on the single runtime thread.

use core::cell::UnsafeCell;
use core::ptr::{self, NonNull};

use crate::abort::{runtime_abort, AbortCode};
use crate::descriptors::LO_STRING_CLASS;
use crate::gc::{MarkBitmap, OffsetTable};
use crate::layout::{Granule, Granules, BLOCK_GRANULES, GRANULE};
use crate::object::{string_data_offset, ClassDescriptor, Object, StringObject};

/// Default heap size (16 MiB), overridable via `LO_HEAP_SIZE` at init.
const DEFAULT_HEAP_SIZE: usize = if cfg!(feature = "gc-stress") {
    4096
} else {
    16 * 1024 * 1024
};

/// The heap's backing memory: zero-initialized granules `[0, end)` from `base`,
/// freed on drop.
pub(crate) struct Region {
    base: NonNull<u8>,
    layout: core::alloc::Layout,
    end: Granule,
}

impl Region {
    fn new(capacity: Granules) -> Region {
        let layout = core::alloc::Layout::from_size_align(capacity.bytes(), GRANULE)
            .expect("valid heap layout");
        // SAFETY: `layout` has non-zero size (at least one block).
        let base = unsafe { std::alloc::alloc_zeroed(layout) };
        let base = NonNull::new(base).unwrap_or_else(|| std::alloc::handle_alloc_error(layout));
        Region {
            base,
            layout,
            end: Granule::ZERO + capacity,
        }
    }

    /// The granule `p` refers to, if `p` lies in `[0, top)`. `None` for null and
    /// for everything outside the heap — `LO_EMPTY_STRING`, descriptors, the
    /// preamble singletons — so the collector never marks, moves or writes them.
    pub(crate) fn locate(&self, p: *mut Object, top: Granule) -> Option<Granule> {
        // Below `base` (null included) wraps to a huge offset.
        let offset = (p as usize).wrapping_sub(self.base.as_ptr() as usize);
        (offset < top.bytes()).then(|| Granule::new((offset / GRANULE) as u32))
    }

    pub(crate) fn addr(&self, g: Granule) -> *mut Object {
        debug_assert!(g <= self.end);
        self.base.as_ptr().wrapping_add(g.bytes()) as *mut Object
    }

    pub(crate) fn end(&self) -> Granule {
        self.end
    }
}

impl Drop for Region {
    fn drop(&mut self) {
        // SAFETY: `base` came from `alloc_zeroed` with this `layout`.
        unsafe { std::alloc::dealloc(self.base.as_ptr(), self.layout) }
    }
}

/// The managed heap and the collector's working state.
pub(crate) struct Heap {
    pub(crate) region: Region,
    /// The bump pointer.
    pub(crate) top: Granule,
    pub(crate) bitmap: MarkBitmap,
    pub(crate) offsets: OffsetTable,
    /// Kept between collections so its capacity is reused.
    pub(crate) mark_stack: Vec<Granule>,
    #[cfg(feature = "gc-verify")]
    pub(crate) verifier: crate::gc::verify::Verifier,
}

impl Heap {
    fn new(capacity: Granules) -> Heap {
        let region = Region::new(capacity);
        let blocks = region.end.blocks_below();
        #[cfg(feature = "gc-verify")]
        unsafe {
            // The entire initial free region has the same invariant as the
            // free tail after a collection. Allocation zeroes claimed bytes.
            ptr::write_bytes(
                region.addr(Granule::ZERO) as *mut u8,
                0xEF,
                capacity.bytes(),
            );
        }
        Heap {
            region,
            top: Granule::ZERO,
            bitmap: MarkBitmap::new(blocks),
            offsets: OffsetTable::new(blocks),
            mark_stack: Vec::new(),
            #[cfg(feature = "gc-verify")]
            verifier: crate::gc::verify::Verifier::new(blocks),
        }
    }

    /// Carve `size` zero-initialized granules and stamp the object header. On a
    /// no-fit, collects and retries once; aborts (exit 137) only if the live set
    /// still leaves no room (ABI §3.1).
    ///
    /// # Safety
    /// `class` must point at a valid, live `ClassDescriptor`. `size` must hold at
    /// least the header.
    unsafe fn alloc(&mut self, size: Granules, class: *const ClassDescriptor) -> *mut Object {
        #[cfg(feature = "gc-verify")]
        self.verifier.register_descriptor(class);
        let obj = match self.bump(size) {
            Some(obj) => obj,
            None => {
                crate::gc::collect(self);
                self.bump(size).unwrap_or_else(|| out_of_memory())
            }
        };
        // Compaction leaves stale bytes above the new top, so zero every slot.
        ptr::write_bytes(obj as *mut u8, 0, size.bytes());
        (*obj).class_descriptor = class;
        (*obj).gc_bits = 0;
        (*obj).flags = 0;
        obj
    }

    fn bump(&mut self, size: Granules) -> Option<*mut Object> {
        let end = self
            .top
            .checked_add(size)
            .filter(|&end| end <= self.region.end)?;
        let obj = self.region.addr(self.top);
        self.top = end;
        Some(obj)
    }
}

struct GlobalHeap(UnsafeCell<Option<Heap>>);

// SAFETY: the runtime is single-threaded (`runtime-abi.md` §1), and the cell is
// reached only through `with_heap`, which never nests.
unsafe impl Sync for GlobalHeap {}

static HEAP: GlobalHeap = GlobalHeap(UnsafeCell::new(None));

/// Run `f` on the global heap. Never call this from inside `f`: a collection
/// runs within the borrow of the allocation that triggered it, and nothing the
/// collector calls comes back here.
pub(crate) fn with_heap<R>(f: impl FnOnce(&mut Option<Heap>) -> R) -> R {
    // SAFETY: single-threaded and never nested, so this is the only borrow.
    f(unsafe { &mut *HEAP.0.get() })
}

fn out_of_memory() -> ! {
    runtime_abort("lo_alloc: out of memory", AbortCode::ABORT_OOM as i32)
}

/// Allocate the heap and its side tables. Called from `lo_runtime_init`.
pub(crate) fn heap_init() {
    let capacity = std::env::var("LO_HEAP_SIZE")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .and_then(|bytes| u32::try_from(bytes / GRANULE).ok())
        .filter(|&n| n >= BLOCK_GRANULES)
        .map_or(Granules::from_bytes(DEFAULT_HEAP_SIZE), Granules::new);
    with_heap(|heap| *heap = Some(Heap::new(capacity)));
}

/// Release the heap. Called from `lo_runtime_shutdown`; idempotent.
pub(crate) fn heap_shutdown() {
    with_heap(|heap| *heap = None);
}

/// # Safety
/// As for [`Heap::alloc`].
unsafe fn alloc(size: Granules, class: *const ClassDescriptor) -> *mut Object {
    with_heap(|heap| match heap {
        Some(heap) => heap.alloc(size, class),
        None => out_of_memory(),
    })
}

/// Allocate a zero-initialized object of `class.instance_size` bytes
/// (`runtime-abi.md` §3.1). Fills in the header. Triggers a collection and
/// retries on a full heap; aborts (exit 137) only on live-set overflow.
///
/// Codegen is responsible for finalizing String-typed fields to `LO_EMPTY_STRING`
/// immediately after this returns (see `runtime-abi.md` §3.1); `lo_alloc` is
/// deliberately type-agnostic.
///
/// # Safety
/// `class` must point at a valid `ClassDescriptor` whose `instance_size` is at
/// least `size_of::<Object>()`.
#[no_mangle]
pub unsafe extern "C" fn lo_alloc(class: *const ClassDescriptor) -> *mut Object {
    alloc(Granules::from_bytes((*class).instance_size as usize), class)
}

/// Allocate a `StringObject` with room for `len` inline bytes, header stamped to
/// `LO_STRING_CLASS` and `length` set. The caller fills the `len` data bytes
/// (already zeroed) at `base + string_data_offset()`.
///
/// This is the internal variable-size string allocator used by the provided
/// `lo_read_string` and by the team-implemented `lo_string_*` ops. `lo_alloc`
/// cannot size a string itself (it only knows the class's fixed `instance_size`).
///
/// # Safety
/// Must be called after `heap_init`. Returns a valid `*mut Object`/`*mut
/// StringObject` or aborts on OOM.
pub(crate) unsafe fn bump_alloc_string(len: u32) -> *mut Object {
    let size = Granules::from_bytes(string_data_offset() + len as usize);
    let obj = alloc(size, &LO_STRING_CLASS);
    let so = obj as *mut StringObject;
    (*so).length = len;
    obj
}

/// Bytes currently allocated (`top`), live or not.
///
/// Test/diagnostic hook (not part of the ABI): lets the GC tests observe heap
/// occupancy drop after a collection reclaims dead objects.
#[doc(hidden)]
pub fn heap_used() -> usize {
    with_heap(|heap| heap.as_ref().map_or(0, |heap| heap.top.bytes()))
}
