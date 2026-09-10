//! I/O surface (`runtime-abi.md` §3.7): four print functions, four read
//! functions, and the EOF probe. Each entry point has one definition with a
//! `cfg` branch per target — native goes through `std::io`, WASM forwards to
//! host-provided imports.
//!
//! **Why `std::io` and not libc on native.** The runbook sketched libc
//! (`printf`/`scanf`/`fgets`/`feof`), but the `libc` crate does not portably
//! expose the `stdin`/`stdout` `FILE*` globals that `fgets`/`feof` need, and
//! `feof`'s "true only after a failed read" semantics don't match the ABI's
//! `lo_eof` ("at end of input *without consuming*"). `std::io`'s `BufRead` gives
//! exactly the peek/consume/`read_until` primitives the read semantics call for,
//! is portable, and needs no extra dependency. The observable behavior — bytes
//! on stdout, tokens/lines off stdin, the documented abort exit codes — is
//! identical. (The Zig and C++ skeletons use their native libc / `<cstdio>`
//! directly, where stdin *is* readily accessible.)

use crate::object::{string_data_offset, Object, StringObject};

// ---------------------------------------------------------------------------
// Native backend: std::io.
// ---------------------------------------------------------------------------
#[cfg(not(target_arch = "wasm32"))]
mod sys {
    use std::io::{self, BufRead, Write};

    /// Write all bytes to stdout and flush (so output is visible before any
    /// later abort, and so test capture sees it immediately).
    pub(super) fn write_out(bytes: &[u8]) {
        let mut out = io::stdout();
        let _ = out.write_all(bytes);
        let _ = out.flush();
    }

    /// Peek the next input byte without consuming it.
    pub(super) fn peek_byte() -> Option<u8> {
        let stdin = io::stdin();
        let mut lock = stdin.lock();
        match lock.fill_buf() {
            Ok(buf) if !buf.is_empty() => Some(buf[0]),
            _ => None,
        }
    }

    /// Consume and return the next input byte.
    pub(super) fn next_byte() -> Option<u8> {
        let b = peek_byte()?;
        io::stdin().lock().consume(1);
        Some(b)
    }

    /// Read up to and including the next newline; returns the bytes with any
    /// trailing `\n` stripped, plus whether end-of-input was hit with no bytes.
    pub(super) fn read_line_bytes() -> (Vec<u8>, bool) {
        let stdin = io::stdin();
        let mut lock = stdin.lock();
        let mut buf = Vec::new();
        let n = lock.read_until(b'\n', &mut buf).unwrap_or(0);
        if n == 0 {
            return (buf, true);
        }
        if buf.last() == Some(&b'\n') {
            buf.pop();
        }
        (buf, false)
    }
}

// ---------------------------------------------------------------------------
// WASM backend: host imports wired by the test harness at instantiation.
// `read_string` uses a two-call protocol (length, then fill) so the runtime can
// size the heap allocation itself.
// ---------------------------------------------------------------------------
#[cfg(target_arch = "wasm32")]
mod sys {
    #[link(wasm_import_module = "host")]
    extern "C" {
        pub(super) fn host_print_int(n: i32);
        pub(super) fn host_print_bool(b: i32);
        pub(super) fn host_print_bytes(ptr: *const u8, len: i32);
        pub(super) fn host_println();
        pub(super) fn host_read_int() -> i32;
        pub(super) fn host_read_bool() -> i32;
        pub(super) fn host_read_line_len() -> i32;
        pub(super) fn host_read_line_into(ptr: *mut u8, max: i32) -> i32;
        pub(super) fn host_eof() -> i32;
    }
}

// ---------------------------------------------------------------------------
// Shared input parsing (native only — WASM delegates token parsing to the host).
// ---------------------------------------------------------------------------
#[cfg(not(target_arch = "wasm32"))]
fn skip_whitespace() {
    while let Some(b) = sys::peek_byte() {
        if b.is_ascii_whitespace() {
            sys::next_byte();
        } else {
            break;
        }
    }
}

// ---------------------------------------------------------------------------
// Print set.
// ---------------------------------------------------------------------------

/// Print an `i32` in decimal (no trailing newline).
#[no_mangle]
pub extern "C" fn lo_print_int(n: i32) {
    #[cfg(not(target_arch = "wasm32"))]
    sys::write_out(n.to_string().as_bytes());
    #[cfg(target_arch = "wasm32")]
    unsafe {
        sys::host_print_int(n);
    }
}

/// Print `true` or `false` (no trailing newline).
#[no_mangle]
pub extern "C" fn lo_print_bool(b: bool) {
    #[cfg(not(target_arch = "wasm32"))]
    sys::write_out(if b { b"true" } else { b"false" });
    #[cfg(target_arch = "wasm32")]
    unsafe {
        sys::host_print_bool(b as i32);
    }
}

/// Print a `StringObject`'s UTF-8 bytes (no trailing newline). A null argument
/// prints nothing.
///
/// # Safety
/// `s`, if non-null, must point at a valid `StringObject` whose inline data holds
/// `length` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn lo_print_string(s: *mut Object) {
    if s.is_null() {
        return;
    }
    let so = s as *const StringObject;
    let len = (*so).length as usize;
    let data = (s as *const u8).add(string_data_offset());
    let slice = core::slice::from_raw_parts(data, len);
    #[cfg(not(target_arch = "wasm32"))]
    sys::write_out(slice);
    #[cfg(target_arch = "wasm32")]
    sys::host_print_bytes(slice.as_ptr(), slice.len() as i32);
}

/// Print a single newline.
#[no_mangle]
pub extern "C" fn lo_println() {
    #[cfg(not(target_arch = "wasm32"))]
    sys::write_out(b"\n");
    #[cfg(target_arch = "wasm32")]
    unsafe {
        sys::host_println();
    }
}

// ---------------------------------------------------------------------------
// Read set.
// ---------------------------------------------------------------------------

/// Read the next integer token, skipping leading whitespace. Aborts with exit
/// 110 on a malformed token, exit 111 on EOF before any integer characters
/// (`runtime-abi.md` §3.7).
#[no_mangle]
pub extern "C" fn lo_read_int() -> i32 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        skip_whitespace();
        // EOF before any integer characters -> exit 111. Any non-whitespace byte
        // present means input exists; failure to parse it is a malformed token
        // (exit 110), not EOF.
        if sys::peek_byte().is_none() {
            crate::abort::runtime_abort(
                "lo_read_int: end of input",
                crate::abort::AbortCode::ABORT_READ_INT_EOF as i32,
            );
        }
        let mut token = String::new();
        if let Some(b) = sys::peek_byte() {
            if b == b'-' || b == b'+' {
                token.push(b as char);
                sys::next_byte();
            }
        }
        let mut saw_digit = false;
        while let Some(b) = sys::peek_byte() {
            if b.is_ascii_digit() {
                token.push(b as char);
                sys::next_byte();
                saw_digit = true;
            } else {
                break;
            }
        }
        if !saw_digit {
            crate::abort::runtime_abort(
                "lo_read_int: malformed token",
                crate::abort::AbortCode::ABORT_READ_INT_MALFORMED as i32,
            );
        }
        match token.parse::<i32>() {
            Ok(n) => n,
            Err(_) => crate::abort::runtime_abort(
                "lo_read_int: malformed token",
                crate::abort::AbortCode::ABORT_READ_INT_MALFORMED as i32,
            ),
        }
    }
    #[cfg(target_arch = "wasm32")]
    unsafe {
        sys::host_read_int()
    }
}

/// Read the next whitespace-delimited token and accept `true` or `false`. Aborts
/// with exit 112 on anything else, including EOF (`runtime-abi.md` §3.7).
#[no_mangle]
pub extern "C" fn lo_read_bool() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        skip_whitespace();
        let mut token = String::new();
        while let Some(b) = sys::peek_byte() {
            if b.is_ascii_whitespace() {
                break;
            }
            token.push(b as char);
            sys::next_byte();
        }
        match token.as_str() {
            "true" => true,
            "false" => false,
            _ => crate::abort::runtime_abort(
                "lo_read_bool: invalid token",
                crate::abort::AbortCode::ABORT_READ_BOOL_MALFORMED as i32,
            ),
        }
    }
    #[cfg(target_arch = "wasm32")]
    unsafe {
        sys::host_read_bool() != 0
    }
}

/// Read up to the next newline (consumed but excluded) and return it as a fresh
/// `StringObject`. Returns the empty string on immediate end-of-input; use
/// `lo_eof` to distinguish EOF from a blank line (`runtime-abi.md` §3.7).
#[no_mangle]
pub extern "C" fn lo_read_string() -> *mut Object {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (bytes, _eof) = sys::read_line_bytes();
        // SAFETY: heap is up after init; we write exactly `bytes.len()` bytes
        // into the inline tail the allocation reserved.
        unsafe {
            let obj = crate::alloc::bump_alloc_string(bytes.len() as u32);
            if !bytes.is_empty() {
                let data = (obj as *mut u8).add(string_data_offset());
                core::ptr::copy_nonoverlapping(bytes.as_ptr(), data, bytes.len());
            }
            obj
        }
    }
    #[cfg(target_arch = "wasm32")]
    unsafe {
        let len = sys::host_read_line_len();
        let len = if len < 0 { 0 } else { len as u32 };
        let obj = crate::alloc::bump_alloc_string(len);
        if len > 0 {
            let data = (obj as *mut u8).add(string_data_offset());
            sys::host_read_line_into(data, len as i32);
        }
        obj
    }
}

/// Return `true` iff stdin is at end-of-input, without consuming any bytes
/// (`runtime-abi.md` §3.7). The standard loop-termination guard.
#[no_mangle]
pub extern "C" fn lo_eof() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        sys::peek_byte().is_none()
    }
    #[cfg(target_arch = "wasm32")]
    unsafe {
        sys::host_eof() != 0
    }
}
