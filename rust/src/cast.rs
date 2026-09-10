//! Type operations (`runtime-abi.md` §3.5).

use crate::abort::{runtime_abort, AbortCode};
use crate::object::{ClassDescriptor, Object};

unsafe fn is_subclass(mut class: *const ClassDescriptor, target: *const ClassDescriptor) -> bool {
    while !class.is_null() {
        if class == target {
            return true;
        }
        class = (*class).parent;
    }
    false
}

unsafe fn class_name(class: *const ClassDescriptor) -> String {
    let bytes = core::slice::from_raw_parts((*class).name, (*class).name_len as usize);
    String::from_utf8_lossy(bytes).into_owned()
}

/// Checked downcast: return `obj` if its class is `target` or a descendant; abort
/// (exit 101) otherwise. Null `obj` returns null.
///
/// # Safety
/// `obj`, if non-null, must point at a valid object; `target` must point at a
/// valid `ClassDescriptor`.
#[no_mangle]
pub unsafe extern "C" fn lo_cast_check(
    obj: *mut Object,
    target: *const ClassDescriptor,
) -> *mut Object {
    if obj.is_null() {
        return obj;
    }
    let class = (*obj).class_descriptor;
    if is_subclass(class, target) {
        return obj;
    }
    runtime_abort(
        &format!(
            "lo_cast_check: cannot cast {} to {}",
            class_name(class),
            class_name(target)
        ),
        AbortCode::ABORT_CAST_FAILURE as i32,
    );
}

/// Return true iff `obj`'s class is `target` or a descendant. Null `obj` yields
/// false; never aborts.
///
/// # Safety
/// `obj`, if non-null, must point at a valid object; `target` must point at a
/// valid `ClassDescriptor`.
#[no_mangle]
pub unsafe extern "C" fn lo_instanceof(obj: *mut Object, target: *const ClassDescriptor) -> bool {
    !obj.is_null() && is_subclass((*obj).class_descriptor, target)
}
