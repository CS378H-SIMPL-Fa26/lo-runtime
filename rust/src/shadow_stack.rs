//! Shadow-stack root tracking (`runtime-abi.md` §3.3).
//!
//! Roots live in a linked list of stack-allocated [`ShadowFrame`]s. Codegen lays
//! out a frame at function entry, registers it, updates `roots[i]` at safepoints,
//! and unregisters at exit. The runtime keeps a single `current_frame` head; push
//! links the new frame's `parent` to it and pop restores the parent.
//!
//! `CURRENT_FRAME` is `static mut` (the runbook sanctions `static mut` for the
//! root-list head). Access is by value only — never `&`/`&mut` of the static —
//! so the `static_mut_refs` lint stays quiet. Invariant: single-threaded; only
//! the functions here mutate the head.

use core::ptr::{self, NonNull};

use crate::layout::Slot;
use crate::object::{shadow_frame_roots_offset, Object, ShadowFrame};

static mut CURRENT_FRAME: *mut ShadowFrame = ptr::null_mut();

/// Register `frame` as the current shadow-stack head.
///
/// # Safety
/// `frame` must point at a valid, live `ShadowFrame` (typically stack-allocated
/// by codegen) that outlives the matching `lo_pop_frame`.
#[no_mangle]
pub unsafe extern "C" fn lo_push_frame(frame: *mut ShadowFrame) {
    (*frame).parent = CURRENT_FRAME;
    CURRENT_FRAME = frame;
}

/// Unregister the current shadow-stack head, restoring its parent.
#[no_mangle]
pub extern "C" fn lo_pop_frame() {
    // SAFETY: by-value read of the head, then a deref of it. A pop with no
    // matching push is a codegen bug; the debug assertion catches it during
    // development (the runbook calls for a defensive non-null check here).
    unsafe {
        let cur = CURRENT_FRAME;
        debug_assert!(!cur.is_null(), "lo_pop_frame called with no current frame");
        CURRENT_FRAME = (*cur).parent;
    }
}

/// Reset the head to null (called from `lo_runtime_init`/`shutdown`).
pub(crate) fn reset() {
    // SAFETY: single-threaded; by-value write of a raw pointer.
    unsafe {
        CURRENT_FRAME = ptr::null_mut();
    }
}

/// Read the current shadow-stack head.
///
/// Not part of the ABI — a debug/test hook so the language-native tests can
/// observe push/pop without reaching into the static directly.
#[doc(hidden)]
pub fn current_frame() -> *mut ShadowFrame {
    // SAFETY: single-threaded; by-value read of a raw pointer.
    unsafe { CURRENT_FRAME }
}

/// Every root slot of every registered frame, walking `current_frame` ->
/// `parent` to null (ABI §3.3). The shadow stack is the complete root set.
pub(crate) fn roots() -> impl Iterator<Item = Slot> {
    // SAFETY: `lo_push_frame`'s contract keeps every registered frame, and so its
    // inline roots, valid until the matching pop.
    let frames = core::iter::successors(NonNull::new(current_frame()), |frame| unsafe {
        NonNull::new((*frame.as_ptr()).parent)
    });
    frames.flat_map(|frame| unsafe {
        let frame = frame.as_ptr();
        let roots = (frame as *mut u8).add(shadow_frame_roots_offset()) as *mut *mut Object;
        (0..(*frame).num_roots as usize).map(move |i| Slot::new(roots.add(i)))
    })
}
