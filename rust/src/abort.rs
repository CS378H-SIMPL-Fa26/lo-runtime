//! Runtime aborts.
//!
//! Native: write the message to stderr and exit with the ABI-specified status
//! code (`runtime-abi.md` §3.8 table). WASM: emit the same message via the host
//! `host.write_stderr` import (§3.7), then execute an `unreachable` trap — so the
//! accompanying stderr is present on both targets, in the same format, for the
//! harness to match on (delta D-B3).

// Host stderr-write import (`runtime-abi.md` §3.7): `host.write_stderr(ptr, len)`
// writes `len` bytes of linear memory at `ptr` to the process's stderr. The WASM
// abort path emits the §3.8 message through it before trapping.
#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "host")]
extern "C" {
    fn host_write_stderr(ptr: *const u8, len: i32);
}

/// Native exit status for each runtime abort (`runtime-abi.md` §3.8 table).
/// Variants are the spec identifiers verbatim (`lo-testing/error-codes.md`
/// § Runtime-abort signals), hence the non-camel-case names.
#[allow(non_camel_case_types)]
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AbortCode {
    ABORT_CAST_FAILURE = 101,
    ABORT_NULL_RECEIVER = 102,
    ABORT_READ_INT_MALFORMED = 110,
    ABORT_READ_INT_EOF = 111,
    ABORT_READ_BOOL_MALFORMED = 112,
    ABORT_STRING_REPEAT_NEGATIVE = 120,
    ABORT_OOM = 137,
}

/// Abort the process with `msg` on stderr and `code` as the exit status (native),
/// or emit `msg` via the host stderr-write import (§3.7) and `unreachable`-trap
/// (WASM). Never returns.
pub(crate) fn runtime_abort(msg: &str, code: i32) -> ! {
    #[cfg(not(target_arch = "wasm32"))]
    {
        eprintln!("{msg}");
        std::process::exit(code);
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = code;
        // Emit the documented §3.8 message before the trap (delta D-B3) so the
        // WASM stderr matches native; the host buffers it and prints it once.
        // SAFETY: `msg` is a valid `&str` (ptr+len of initialized UTF-8 bytes);
        // the import only reads `len` bytes of linear memory at `ptr`.
        unsafe {
            host_write_stderr(msg.as_ptr(), msg.len() as i32);
        }
        core::arch::wasm32::unreachable()
    }
}

/// Abort on a null receiver at method dispatch (`runtime-abi.md` §3.8). Codegen
/// emits a null check before each dispatch and calls this on failure with a
/// pointer to the method's name. Exit status 102 on native; `unreachable` on
/// WASM.
///
/// # Safety
/// `method_name`, if non-null, must point at `method_name_len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn lo_abort_null_receiver(method_name: *const u8, method_name_len: u32) -> ! {
    let name = if method_name.is_null() {
        "<unknown>"
    } else {
        let bytes = core::slice::from_raw_parts(method_name, method_name_len as usize);
        core::str::from_utf8(bytes).unwrap_or("<invalid-utf8>")
    };
    runtime_abort(
        &format!("lo_abort_null_receiver: cannot dispatch {name}"),
        AbortCode::ABORT_NULL_RECEIVER as i32,
    )
}
