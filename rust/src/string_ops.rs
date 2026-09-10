//! String operations (`runtime-abi.md` §3.2).

use crate::abort::{runtime_abort, AbortCode};
use crate::alloc::bump_alloc_string;
use crate::object::{string_data_offset, Object, StringObject};

unsafe fn bytes<'a>(s: *mut Object) -> &'a [u8] {
    let len = (*(s as *const StringObject)).length as usize;
    core::slice::from_raw_parts((s as *const u8).add(string_data_offset()), len)
}

// Allocation may collect and move heap objects, so every op reads its inputs
// into `src` before calling this.
unsafe fn new_string(src: &[u8]) -> *mut Object {
    if src.len() > u32::MAX as usize {
        runtime_abort("lo_alloc: out of memory", AbortCode::ABORT_OOM as i32);
    }
    let obj = bump_alloc_string(src.len() as u32);
    if !src.is_empty() {
        let data = (obj as *mut u8).add(string_data_offset());
        core::ptr::copy_nonoverlapping(src.as_ptr(), data, src.len());
    }
    obj
}

/// Construct a string from `len` raw UTF-8 bytes (copied; caller owns the
/// source).
///
/// # Safety
/// `bytes` must point at `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn lo_string_new(bytes: *const u8, len: u32) -> *mut Object {
    let src: Vec<u8> = if len == 0 {
        Vec::new()
    } else {
        core::slice::from_raw_parts(bytes, len as usize).to_vec()
    };
    new_string(&src)
}

/// Return a new string `a + b`.
///
/// # Safety
/// `a` and `b` must point at valid `StringObject`s.
#[no_mangle]
pub unsafe extern "C" fn lo_string_concat(a: *mut Object, b: *mut Object) -> *mut Object {
    let mut src = bytes(a).to_vec();
    src.extend_from_slice(bytes(b));
    new_string(&src)
}

/// Return a new string: `s` repeated `n` times. Aborts (exit 120) on negative
/// `n`.
///
/// # Safety
/// `s` must point at a valid `StringObject`.
#[no_mangle]
pub unsafe extern "C" fn lo_string_repeat(s: *mut Object, n: i32) -> *mut Object {
    if n < 0 {
        runtime_abort(
            &format!("lo_string_repeat: negative count {n}"),
            AbortCode::ABORT_STRING_REPEAT_NEGATIVE as i32,
        );
    }
    let src = bytes(s);
    if src.len().checked_mul(n as usize).is_none() {
        runtime_abort("lo_alloc: out of memory", AbortCode::ABORT_OOM as i32);
    }
    new_string(&src.repeat(n as usize))
}

/// Compare two strings, returning negative / zero / positive by lexicographic
/// UTF-8 byte ordering.
///
/// # Safety
/// `a` and `b` must point at valid `StringObject`s.
#[no_mangle]
pub unsafe extern "C" fn lo_string_compare(a: *mut Object, b: *mut Object) -> i32 {
    bytes(a).cmp(bytes(b)) as i32
}

/// Return a new string with codepoints reversed.
///
/// # Safety
/// `s` must point at a valid `StringObject`.
#[no_mangle]
pub unsafe extern "C" fn lo_string_reverse(s: *mut Object) -> *mut Object {
    let src = match core::str::from_utf8(bytes(s)) {
        Ok(text) => text.chars().rev().collect::<String>().into_bytes(),
        Err(_) => bytes(s).iter().rev().copied().collect(),
    };
    new_string(&src)
}
