//! Independent structural checks for the sliding heap (`--features gc-verify`).
//!
//! Descriptors enter the trusted set only at the allocation API; immortal
//! objects enter only through explicit registration. Never dereference a root
//! or field to decide whether it is valid. First walk bounded heap objects and
//! record their exact starts, then check edges against those starts/statics.
//! The scratch bitmap is allocated at heap init, not during collection.
//!
//! This checks structure, not program intent: a missing root, or a stale
//! reference that happens to equal another valid object start, needs graph/data
//! preservation tests. The ABI's shadow-frame chain and registered immutable
//! descriptor storage are trusted; this is not a general memory-safety sandbox.

use core::mem::{align_of, size_of};
use core::ptr;

use super::MarkBitmap;
use crate::alloc::{Heap, Region};
use crate::descriptors::{LO_BOOL_BOX_CLASS, LO_EMPTY_STRING, LO_INT_BOX_CLASS, LO_STRING_CLASS};
use crate::layout::{Granule, Granules, GRANULE};
use crate::object::{string_data_offset, ClassDescriptor, Object, StringObject};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Error {
    pub kind: &'static str,
    pub address: usize,
}

fn error(kind: &'static str, address: usize) -> Error {
    Error { kind, address }
}

struct Descriptor {
    address: *const ClassDescriptor,
    size: usize,
    offsets_address: *const u32,
    offsets: Vec<u32>,
}

struct Static {
    address: *mut Object,
    descriptor: *const ClassDescriptor,
    size: usize,
}

pub(crate) struct Verifier {
    descriptors: Vec<Descriptor>,
    statics: Vec<Static>,
    starts: MarkBitmap,
}

impl Verifier {
    pub(crate) fn new(blocks: usize) -> Self {
        let mut verifier = Self {
            descriptors: Vec::new(),
            statics: Vec::new(),
            starts: MarkBitmap::new(blocks),
        };
        // SAFETY: runtime-owned immutable descriptors and singleton.
        unsafe {
            for class in [&LO_STRING_CLASS, &LO_INT_BOX_CLASS, &LO_BOOL_BOX_CLASS] {
                verifier.register_descriptor(class);
            }
            verifier.register_static(ptr::addr_of!(LO_EMPTY_STRING) as *mut Object);
        }
        verifier
    }

    /// Called only at a trusted API boundary, never learned from heap headers.
    /// The caller guarantees valid immutable descriptor and offset storage.
    pub(crate) unsafe fn register_descriptor(&mut self, address: *const ClassDescriptor) {
        if self.descriptors.iter().any(|d| d.address == address) {
            return;
        }
        assert!(
            !address.is_null(),
            "gc verifier: null descriptor registration"
        );
        let class = &*address;
        assert!(
            class.instance_size as usize >= size_of::<Object>(),
            "gc verifier: descriptor smaller than object header"
        );
        let offsets = if class.pointer_count == 0 {
            Vec::new()
        } else {
            assert!(
                !class.pointer_offsets.is_null(),
                "gc verifier: null offset table"
            );
            core::slice::from_raw_parts(class.pointer_offsets, class.pointer_count as usize)
                .to_vec()
        };
        for &offset in &offsets {
            let offset = offset as usize;
            assert!(
                offset >= size_of::<Object>()
                    && offset % align_of::<*mut Object>() == 0
                    && offset
                        .checked_add(size_of::<*mut Object>())
                        .is_some_and(|end| end <= class.instance_size as usize),
                "gc verifier: invalid pointer field offset"
            );
        }
        self.descriptors.push(Descriptor {
            address,
            size: class.instance_size as usize,
            offsets_address: class.pointer_offsets,
            offsets,
        });
    }

    pub(crate) unsafe fn register_static(&mut self, address: *mut Object) {
        if self.statics.iter().any(|s| s.address == address) {
            return;
        }
        assert!(!address.is_null(), "gc verifier: null static registration");
        let descriptor = (*address).class_descriptor;
        self.register_descriptor(descriptor);
        let size = if ptr::eq(descriptor, &LO_STRING_CLASS) {
            string_data_offset()
                .checked_add((*(address as *const StringObject)).length as usize)
                .expect("gc verifier: static string size overflow")
        } else {
            (*descriptor).instance_size as usize
        };
        self.statics.push(Static {
            address,
            descriptor,
            size,
        });
    }

    /// Read a bounded header; descriptor identity is checked BEFORE dereference.
    unsafe fn object(
        &self,
        address: *mut Object,
        available: usize,
    ) -> Result<(usize, &Descriptor), Error> {
        let at = address as usize;
        if available < size_of::<Object>() {
            return Err(error("truncated object header", at));
        }
        let header = &*address;
        let descriptor = self
            .descriptors
            .iter()
            .find(|d| d.address == header.class_descriptor)
            .ok_or_else(|| error("unknown descriptor", at))?;
        // The registry retains a trusted static descriptor address. Do not
        // follow a possibly corrupted offset-table pointer from its contents.
        let class = &*descriptor.address;
        if class.instance_size as usize != descriptor.size
            || class.pointer_offsets != descriptor.offsets_address
            || class.pointer_count as usize != descriptor.offsets.len()
        {
            return Err(error("descriptor metadata changed", at));
        }
        for (i, &offset) in descriptor.offsets.iter().enumerate() {
            if *descriptor.offsets_address.add(i) != offset {
                return Err(error("descriptor offsets changed", at));
            }
        }
        if header.gc_bits != 0 {
            return Err(error("unexpected GC header bits", at));
        }
        let size = if ptr::eq(descriptor.address, &LO_STRING_CLASS) {
            if available < string_data_offset() {
                return Err(error("truncated string header", at));
            }
            string_data_offset()
                .checked_add((*(address as *const StringObject)).length as usize)
                .ok_or_else(|| error("string size overflow", at))?
        } else {
            descriptor.size
        };
        if size > available {
            return Err(error("object extends beyond allocation frontier", at));
        }
        Ok((size, descriptor))
    }

    fn reference(
        &self,
        region: &Region,
        top: Granule,
        value: *mut Object,
        slot: usize,
    ) -> Result<(), Error> {
        if value.is_null() {
            return Ok(());
        }
        let offset = (value as usize).wrapping_sub(region.addr(Granule::ZERO) as usize);
        if offset < region.end().bytes() {
            if offset >= top.bytes() {
                return Err(error("reference into free heap", slot));
            }
            if offset % GRANULE != 0
                || !self
                    .starts
                    .is_marked(Granule::new((offset / GRANULE) as u32))
            {
                return Err(error("reference is not an object start", slot));
            }
            return Ok(());
        }
        if self.statics.iter().any(|s| s.address == value) {
            Ok(())
        } else {
            Err(error("unregistered non-heap reference", slot))
        }
    }

    fn check(&mut self, region: &Region, top: Granule) -> Result<(), Error> {
        if top > region.end() {
            return Err(error("allocation frontier outside heap", top.bytes()));
        }
        self.starts.clear_below(region.end());
        let mut at = Granule::ZERO;
        while at < top {
            // SAFETY: `at` is aligned and below the bounded allocation frontier;
            // object() checks available bytes before reading each header part.
            let (size, _) = unsafe { self.object(region.addr(at), top.bytes() - at.bytes())? };
            let size = Granules::from_bytes(size);
            let end = at
                .checked_add(size)
                .filter(|&end| end <= top)
                .ok_or_else(|| error("rounded object exceeds frontier", at.bytes()))?;
            self.starts.mark(at, Granules::new(1));
            at = end;
        }
        // Now every valid start is known, including forward references/cycles.
        at = Granule::ZERO;
        while at < top {
            let object = region.addr(at);
            let (size, descriptor) = unsafe { self.object(object, top.bytes() - at.bytes())? };
            for &offset in &descriptor.offsets {
                // SAFETY: descriptor offsets were bounded/aligned on registration.
                let slot =
                    unsafe { (object as *mut u8).add(offset as usize) as *const *mut Object };
                self.reference(region, top, unsafe { *slot }, slot as usize)?;
            }
            at = at + Granules::from_bytes(size);
        }
        for root in crate::shadow_stack::roots() {
            let value = root.get();
            self.reference(region, top, value, value as usize)?;
        }
        for static_ in &self.statics {
            let (size, descriptor) = unsafe { self.object(static_.address, static_.size)? };
            if descriptor.address != static_.descriptor || size != static_.size {
                return Err(error("static object changed", static_.address as usize));
            }
            for &offset in &descriptor.offsets {
                let value = unsafe {
                    *((static_.address as *const u8).add(offset as usize) as *const *mut Object)
                };
                // Immortal objects are not traced by this collector. Their
                // fields can only refer to null or other registered statics.
                if !value.is_null() && !self.statics.iter().any(|s| s.address == value) {
                    return Err(error(
                        "static object has non-static edge",
                        static_.address as usize,
                    ));
                }
            }
        }
        // SAFETY: the free tail is within initialized backing storage.
        let free = unsafe {
            core::slice::from_raw_parts(
                region.addr(top) as *const u8,
                region.end().bytes() - top.bytes(),
            )
        };
        if let Some(offset) = free.iter().position(|&byte| byte != 0xEF) {
            return Err(error(
                "free space is not poisoned",
                region.addr(top) as usize + offset,
            ));
        }
        Ok(())
    }
}

pub(crate) fn check(heap: &mut Heap) -> Result<(), Error> {
    heap.verifier.check(&heap.region, heap.top)
}

pub(crate) fn assert_valid(heap: &mut Heap, phase: &str) {
    if let Err(error) = check(heap) {
        crate::abort::runtime_abort(
            &format!(
                "lo_gc_verify: {phase}: {} at {:#x}",
                error.kind, error.address
            ),
            134,
        );
    }
}
