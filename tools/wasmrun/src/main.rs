//! `wasmrun` — the WASM host harness for the CS 378H course toolchain.
//!
//! The LO runtime's WASM build imports a small set of `host` I/O functions
//! (`runtime-abi.md` §3.7; not WASI), so running a linked LO module needs an
//! embedder that supplies them — that is this binary.
//!
//! Usage: `wasmrun <module.wasm>`  (stdin is piped to the guest's reads;
//! stdout/stderr pass through). The module must export `lo_entry () -> i32`
//! (the synthesized entry; its result is the LO program's exit status) and a
//! `memory`.
//!
//! Exit status:
//! - the value `lo_entry` returns, on normal completion; or
//! - a runtime-abort code on a guest trap. Read failures (110/111/112) are
//!   detected here in the host (the WASM read path delegates token parsing to
//!   the host, `io.rs`), so we know the exact kind and message. Runtime-originated
//!   traps (in `lo_alloc`/`lo_cast_check`/`lo_string_repeat`/
//!   `lo_abort_null_receiver`) are classified to their exit *code* by the trapping
//!   frame in the wasmtime backtrace; the *message* is emitted by the runtime via
//!   the `host_write_stderr` import before the trap and printed
//!   verbatim. Every such abort emits its documented
//!   `runtime-abi.md` §3.8 message; the hard-coded fallbacks below are retained
//!   only as a defensive last resort should emission ever be missing.

use std::io::{Read, Write};

use wasmtime::*;

/// A read failure raised by a `host_read_*` function: carries the ABI exit code
/// and the message to emit, and propagates out as a guest trap.
#[derive(Debug)]
struct ReadAbort {
    code: i32,
    msg: &'static str,
}
impl std::fmt::Display for ReadAbort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.msg)
    }
}
impl std::error::Error for ReadAbort {}

/// Host-side stdin state. Token reads and the two-call line protocol operate
/// over a single buffered slurp of stdin.
struct Ctx {
    buf: Vec<u8>,
    pos: usize,
    /// Pending line for the `host_read_line_len` → `host_read_line_into`
    /// two-call protocol (`io.rs` `lo_read_string`).
    pending_line: Option<Vec<u8>>,
    /// The abort message the runtime emitted via `host_write_stderr` before
    /// trapping. When present, `abort_exit` prints it instead of
    /// reconstructing a generic message from the backtrace.
    abort_msg: Option<Vec<u8>>,
}

impl Ctx {
    fn skip_ws(&mut self) {
        while self.pos < self.buf.len() && self.buf[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }
    fn next_token(&mut self) -> Option<String> {
        self.skip_ws();
        if self.pos >= self.buf.len() {
            return None;
        }
        let start = self.pos;
        while self.pos < self.buf.len() && !self.buf[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
        Some(String::from_utf8_lossy(&self.buf[start..self.pos]).into_owned())
    }
    fn at_eof(&self) -> bool {
        self.pos >= self.buf.len()
    }
    fn take_line(&mut self) -> Vec<u8> {
        let start = self.pos;
        while self.pos < self.buf.len() && self.buf[self.pos] != b'\n' {
            self.pos += 1;
        }
        let line = self.buf[start..self.pos].to_vec();
        if self.pos < self.buf.len() {
            self.pos += 1; // consume the newline
        }
        line
    }
}

fn memory(c: &mut Caller<'_, Ctx>) -> Memory {
    c.get_export("memory")
        .and_then(|e| e.into_memory())
        .expect("linked module must export `memory`")
}

fn main() {
    let path = match std::env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!("usage: wasmrun <module.wasm>");
            std::process::exit(2);
        }
    };
    std::process::exit(run(&path));
}

fn run(path: &str) -> i32 {
    let engine = Engine::default();
    let module = match Module::from_file(&engine, path) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("wasmrun: cannot load `{path}`: {e}");
            return 2;
        }
    };
    let mut stdin_buf = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut stdin_buf);
    let mut store = Store::new(&engine, Ctx { buf: stdin_buf, pos: 0, pending_line: None, abort_msg: None });

    let mut linker = Linker::new(&engine);
    wire_host(&mut linker);

    let inst = match linker.instantiate(&mut store, &module) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("wasmrun: instantiation failed: {e}");
            return 2;
        }
    };
    let entry = match inst.get_typed_func::<(), i32>(&mut store, "lo_entry") {
        Ok(f) => f,
        Err(e) => {
            eprintln!("wasmrun: module has no `lo_entry () -> i32`: {e}");
            return 2;
        }
    };

    match entry.call(&mut store, ()) {
        Ok(code) => {
            let _ = std::io::stdout().flush();
            code
        }
        Err(err) => {
            let _ = std::io::stdout().flush();
            let emitted = store.data_mut().abort_msg.take();
            abort_exit(err, emitted)
        }
    }
}

/// Map a guest trap to the documented runtime-abort exit code + stderr message.
/// `emitted` is the message the runtime wrote via `host_write_stderr` before the
/// trap; when present it is the authoritative message and is printed
/// verbatim, since only the runtime knows the parameterized detail (e.g. the
/// class names in `lo_cast_check: cannot cast <from> to <to>`).
fn abort_exit(err: anyhow::Error, emitted: Option<Vec<u8>>) -> i32 {
    // A host read-failure carries its own code/message.
    if let Some(ra) = err.downcast_ref::<ReadAbort>() {
        eprintln!("{}", ra.msg);
        return ra.code;
    }
    // Otherwise it is a runtime trap. The exit code is classified from the
    // trapping frame in the backtrace. The message is the one the runtime emitted
    // via `host_write_stderr` before trapping (now every abort kind emits);
    // the documented §3.8 fallbacks below fire only if emission was somehow absent.
    let bt = format!("{err:?}");
    let (code, fallback): (i32, &str) = if bt.contains("lo_abort_null_receiver") {
        (102, "lo_abort_null_receiver: cannot dispatch <method>")
    } else if bt.contains("lo_cast_check") {
        (101, "lo_cast_check: cast failure")
    } else if bt.contains("lo_string_repeat") {
        (120, "lo_string_repeat: negative count")
    } else if bt.contains("lo_alloc") {
        (137, "lo_alloc: out of memory")
    } else {
        // Unknown trap. If the runtime emitted a message, surface it; else dump
        // the backtrace for debugging. Non-zero, distinct exit.
        match &emitted {
            Some(m) => {
                let _ = std::io::stderr().write_all(m);
                eprintln!();
            }
            None => eprintln!("wasmrun: unclassified guest trap:\n{bt}"),
        }
        return 134;
    };
    match emitted {
        Some(m) => {
            let _ = std::io::stderr().write_all(&m);
            eprintln!();
        }
        None => eprintln!("{fallback}"),
    }
    code
}

/// Write a print-family payload to the stream the guest's `to_stderr` selector
/// names: `0` stdout, `1` stderr (`runtime-abi.md` §3.7). Other values are
/// reserved — codegen never emits them, and they fall back to stdout here.
/// Each write is flushed immediately, on either stream, so a program
/// interleaving `out` and `err` lands its bytes on each stream in issue order
/// (and so the harness's separate stdout/stderr captures are both complete
/// even if the guest later traps).
fn host_write(bytes: &[u8], to_stderr: i32) {
    if to_stderr == 1 {
        let mut err = std::io::stderr();
        let _ = err.write_all(bytes);
        let _ = err.flush();
    } else {
        let mut out = std::io::stdout();
        let _ = out.write_all(bytes);
        let _ = out.flush();
    }
}

fn wire_host(linker: &mut Linker<Ctx>) {
    linker
        .func_wrap("host", "host_print_int", |_c: Caller<'_, Ctx>, n: i32, to_stderr: i32| {
            host_write(n.to_string().as_bytes(), to_stderr);
        })
        .unwrap();
    linker
        .func_wrap("host", "host_print_bool", |_c: Caller<'_, Ctx>, b: i32, to_stderr: i32| {
            host_write(if b != 0 { b"true".as_slice() } else { b"false".as_slice() }, to_stderr);
        })
        .unwrap();
    linker
        .func_wrap("host", "host_print_bytes", |mut c: Caller<'_, Ctx>, ptr: i32, len: i32, to_stderr: i32| {
            let m = memory(&mut c);
            let mut buf = vec![0u8; len.max(0) as usize];
            m.read(&c, ptr as usize, &mut buf).unwrap_or(());
            host_write(&buf, to_stderr);
        })
        .unwrap();
    linker
        .func_wrap("host", "host_println", |_c: Caller<'_, Ctx>, to_stderr: i32| {
            host_write(b"\n", to_stderr);
        })
        .unwrap();
    // §3.7 stderr-write import: the runtime uses it to emit an abort message
    // before trapping. Buffer it in `Ctx` so `abort_exit` prints it once,
    // after stdout is flushed and in the documented format.
    linker
        .func_wrap("host", "host_write_stderr", |mut c: Caller<'_, Ctx>, ptr: i32, len: i32| {
            let m = memory(&mut c);
            let mut buf = vec![0u8; len.max(0) as usize];
            m.read(&c, ptr as usize, &mut buf).unwrap_or(());
            c.data_mut().abort_msg = Some(buf);
        })
        .unwrap();
    // Reads: token parsing lives here on WASM (io.rs delegates). A failure
    // returns a ReadAbort error, which wasmtime propagates as a guest trap.
    linker
        .func_wrap("host", "host_read_int", |mut c: Caller<'_, Ctx>| -> Result<i32> {
            let ctx = c.data_mut();
            match ctx.next_token() {
                None => Err(ReadAbort { code: 111, msg: "lo_read_int: end of input" }.into()),
                Some(t) => t
                    .parse::<i32>()
                    .map_err(|_| ReadAbort { code: 110, msg: "lo_read_int: malformed token" }.into()),
            }
        })
        .unwrap();
    linker
        .func_wrap("host", "host_read_bool", |mut c: Caller<'_, Ctx>| -> Result<i32> {
            let ctx = c.data_mut();
            match ctx.next_token().as_deref() {
                Some("true") => Ok(1),
                Some("false") => Ok(0),
                _ => Err(ReadAbort { code: 112, msg: "lo_read_bool: invalid token" }.into()),
            }
        })
        .unwrap();
    linker
        .func_wrap("host", "host_read_line_len", |mut c: Caller<'_, Ctx>| -> i32 {
            let ctx = c.data_mut();
            let line = ctx.take_line();
            let len = line.len() as i32;
            ctx.pending_line = Some(line);
            len
        })
        .unwrap();
    linker
        .func_wrap("host", "host_read_line_into", |mut c: Caller<'_, Ctx>, ptr: i32, max: i32| -> i32 {
            let line = c.data_mut().pending_line.take().unwrap_or_default();
            let n = line.len().min(max.max(0) as usize);
            let m = memory(&mut c);
            let _ = m.write(&mut c, ptr as usize, &line[..n]);
            n as i32
        })
        .unwrap();
    linker
        .func_wrap("host", "host_eof", |c: Caller<'_, Ctx>| -> i32 {
            i32::from(c.data().at_eof())
        })
        .unwrap();
}
