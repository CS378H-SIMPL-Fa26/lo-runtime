//! Collector-side views of heap objects: sizes in granules, per-object layouts,
//! and the slots that hold references.
//!
//! These are crate-internal (not part of the C ABI). The collector and the
//! allocator use them so that object sizes and reference locations are computed
//! in exactly one place.

use core::ptr::{self, NonNull};

use crate::descriptors::LO_STRING_CLASS;
use crate::object::{string_data_offset, Object, StringObject};

/// The heap's unit of allocation, in bytes. Every object occupies a whole number
/// of granules, on both targets.
pub(crate) const GRANULE: usize = 8;

/// A size measured in granules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Granules(u32);

impl Granules {
    /// The fewest granules that hold `bytes` bytes.
    pub(crate) fn from_bytes(bytes: usize) -> Granules {
        let n = bytes.div_ceil(GRANULE);
        Granules(u32::try_from(n).expect("object size fits in u32 granules"))
    }

    pub(crate) fn bytes(self) -> usize {
        self.0 as usize * GRANULE
    }
}

/// What the collector needs to know about one object: how big it is and where
/// its references live. Read once per object, while its header is intact.
#[derive(Clone, Copy)]
pub(crate) struct Layout {
    pub size: Granules,
    /// Byte offsets of the object's reference fields.
    pub pointers: &'static [u32],
}

impl Layout {
    /// The reference slots of `obj`, which has this layout.
    ///
    /// # Safety
    /// `obj` must point at a valid object with this layout, and must stay at that
    /// address while the returned slots are used.
    pub(crate) unsafe fn pointer_slots(&self, obj: *mut Object) -> impl Iterator<Item = Slot> {
        let base = obj as *mut u8;
        self.pointers
            .iter()
            .map(move |&off| Slot::new(base.add(off as usize) as *mut *mut Object))
    }
}

/// The layout of `obj`. Strings are variable-size: their inline UTF-8 tail
/// starts at `offset_of(StringObject, data)` (20 on 64-bit), **not** `size_of`
/// (`runtime-abi.md` §2.2).
///
/// # Safety
/// `obj` must point at a valid object whose `class_descriptor` is intact, and
/// the descriptor (with its `pointer_offsets`) must be static data.
pub(crate) unsafe fn layout(obj: *const Object) -> Layout {
    let class = &*(*obj).class_descriptor;
    if ptr::eq(class, &LO_STRING_CLASS) {
        let length = (*(obj as *const StringObject)).length as usize;
        return Layout {
            size: Granules::from_bytes(string_data_offset() + length),
            pointers: &[],
        };
    }
    // The built-in descriptors carry a null `pointer_offsets` with a zero count,
    // and `from_raw_parts` requires a non-null pointer even for an empty slice.
    let pointers = if class.pointer_count == 0 {
        &[]
    } else {
        core::slice::from_raw_parts(class.pointer_offsets, class.pointer_count as usize)
    };
    Layout {
        size: Granules::from_bytes(class.instance_size as usize),
        pointers,
    }
}

/// A place that holds a reference: a shadow-stack root or an object's
/// reference field. The reference itself may be null or point outside the heap.
#[derive(Clone, Copy)]
pub(crate) struct Slot(NonNull<*mut Object>);

impl Slot {
    /// # Safety
    /// `p` must point at a readable and writable reference that stays valid for
    /// every use of the returned slot.
    pub(crate) unsafe fn new(p: *mut *mut Object) -> Slot {
        Slot(NonNull::new_unchecked(p))
    }

    pub(crate) fn get(self) -> *mut Object {
        // SAFETY: `Slot::new`'s contract keeps the slot valid while in use.
        unsafe { *self.0.as_ptr() }
    }

    pub(crate) fn set(self, value: *mut Object) {
        // SAFETY: as in `get`.
        unsafe { *self.0.as_ptr() = value }
    }
}
