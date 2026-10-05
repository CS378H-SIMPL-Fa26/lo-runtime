//! I/O surface (`runtime-abi.md` §3.7). Each entry point branches at comptime on
//! the target: native goes through `std.io`, WASM forwards to host imports.
//!
//! Like the Rust skeleton, native I/O uses the standard library rather than libc:
//! `std.io` needs no libc linkage and gives correct peek / EOF semantics (the
//! ABI's `lo_eof` is "at end-of-input *without consuming*", which `feof` does not
//! provide). Observable behavior and abort exit codes are identical.

const std = @import("std");
const builtin = @import("builtin");

const object = @import("object.zig");
const alloc = @import("alloc.zig");
const abort = @import("abort.zig");
const Object = object.Object;
const StringObject = object.StringObject;

const is_wasm = builtin.cpu.arch.isWasm();

// --- WASM host imports (wired by the harness at instantiation) --------------
extern "host" fn host_print_int(n: i32, to_stderr: i32) void;
extern "host" fn host_print_bool(b: i32, to_stderr: i32) void;
extern "host" fn host_print_bytes(ptr: [*]const u8, len: i32, to_stderr: i32) void;
extern "host" fn host_println(to_stderr: i32) void;
extern "host" fn host_read_int() i32;
extern "host" fn host_read_bool() i32;
extern "host" fn host_read_line_len() i32;
extern "host" fn host_read_line_into(ptr: [*]u8, max: i32) i32;
extern "host" fn host_eof() i32;

// --- Native helpers (analyzed only on native, via the comptime branches) ----
// A single byte of pushback gives peek/EOF without a persistent buffer. Reads
// are one byte at a time — simple and correct; test inputs are tiny.
var peeked: i32 = -1; // -1 = empty

fn rawReadByte() ?u8 {
    var b: [1]u8 = undefined;
    const n = std.io.getStdIn().read(&b) catch return null;
    if (n == 0) return null;
    return b[0];
}

fn peekByte() ?u8 {
    if (peeked >= 0) return @intCast(peeked);
    const b = rawReadByte() orelse return null;
    peeked = b;
    return b;
}

fn nextByte() ?u8 {
    if (peeked >= 0) {
        const b: u8 = @intCast(peeked);
        peeked = -1;
        return b;
    }
    return rawReadByte();
}

fn skipWhitespace() void {
    while (peekByte()) |b| {
        if (std.ascii.isWhitespace(b)) {
            _ = nextByte();
        } else break;
    }
}

fn writeOut(bytes: []const u8) void {
    std.io.getStdOut().writeAll(bytes) catch {};
}

/// The `to_stderr == 1` half of the print family's destination selector
/// (`runtime-abi.md` §3.7). Same discipline as `writeOut` — an unbuffered
/// `writeAll` straight at the fd, so interleaved `out`/`err` output lands on
/// each stream in issue order.
fn writeErr(bytes: []const u8) void {
    std.io.getStdErr().writeAll(bytes) catch {};
}

/// Route `bytes` to the stream the print family's selector names: `0` stdout,
/// `1` stderr. Other values are reserved (`runtime-abi.md` §3.7); codegen never
/// emits them, and they fall back to stdout here.
fn writeSel(bytes: []const u8, to_stderr: i32) void {
    if (to_stderr == 1) writeErr(bytes) else writeOut(bytes);
}

// --- Print set --------------------------------------------------------------

/// Print an `i32` in decimal (no trailing newline) to the stream `to_stderr`
/// selects: `0` stdout, `1` stderr (`runtime-abi.md` §3.7).
pub export fn lo_print_int(n: i32, to_stderr: i32) void {
    if (comptime is_wasm) {
        host_print_int(n, to_stderr);
    } else {
        var buf: [16]u8 = undefined;
        const s = std.fmt.bufPrint(&buf, "{d}", .{n}) catch unreachable;
        writeSel(s, to_stderr);
    }
}

/// Print `true` or `false` (no trailing newline) to the stream `to_stderr`
/// selects: `0` stdout, `1` stderr (`runtime-abi.md` §3.7).
pub export fn lo_print_bool(b: bool, to_stderr: i32) void {
    if (comptime is_wasm) {
        host_print_bool(if (b) 1 else 0, to_stderr);
    } else {
        writeSel(if (b) "true" else "false", to_stderr);
    }
}

/// Print a `StringObject`'s UTF-8 bytes (no trailing newline) to the stream
/// `to_stderr` selects: `0` stdout, `1` stderr (`runtime-abi.md` §3.7). Null
/// prints nothing.
pub export fn lo_print_string(s: ?*Object, to_stderr: i32) void {
    const obj_ptr = s orelse return;
    const so: *const StringObject = @ptrCast(@alignCast(obj_ptr));
    const len = so.length;
    const data: [*]const u8 = @ptrFromInt(@intFromPtr(obj_ptr) + object.stringDataOffset());
    if (comptime is_wasm) {
        host_print_bytes(data, @intCast(len), to_stderr);
    } else {
        writeSel(data[0..len], to_stderr);
    }
}

/// Print a single newline to the stream `to_stderr` selects: `0` stdout, `1`
/// stderr (`runtime-abi.md` §3.7). The selector is this entry point's only
/// argument — it is otherwise nullary.
pub export fn lo_println(to_stderr: i32) void {
    if (comptime is_wasm) {
        host_println(to_stderr);
    } else {
        writeSel("\n", to_stderr);
    }
}

// --- Read set ---------------------------------------------------------------

/// Read the next integer token, skipping leading whitespace. Aborts exit 110 on a
/// malformed token, exit 111 on EOF before any integer characters.
pub export fn lo_read_int() i32 {
    if (comptime is_wasm) {
        return host_read_int();
    } else {
        skipWhitespace();
        // EOF before any integer characters -> 111. A non-whitespace byte present
        // means input exists; failure to parse it is malformed (110), not EOF.
        if (peekByte() == null) {
            abort.runtimeAbort("lo_read_int: end of input", 111);
        }
        var buf: [32]u8 = undefined;
        var idx: usize = 0;
        if (peekByte()) |b| {
            if (b == '-' or b == '+') {
                buf[idx] = b;
                idx += 1;
                _ = nextByte();
            }
        }
        var saw_digit = false;
        while (peekByte()) |b| {
            if (b >= '0' and b <= '9') {
                if (idx < buf.len) {
                    buf[idx] = b;
                    idx += 1;
                }
                _ = nextByte();
                saw_digit = true;
            } else break;
        }
        if (!saw_digit) {
            abort.runtimeAbort("lo_read_int: malformed token", 110);
        }
        return std.fmt.parseInt(i32, buf[0..idx], 10) catch
            abort.runtimeAbort("lo_read_int: malformed token", 110);
    }
}

/// Read the next whitespace-delimited token and accept `true` or `false`. Aborts
/// exit 112 on anything else, including EOF.
pub export fn lo_read_bool() bool {
    if (comptime is_wasm) {
        return host_read_bool() != 0;
    } else {
        skipWhitespace();
        var buf: [8]u8 = undefined;
        var idx: usize = 0;
        while (peekByte()) |b| {
            if (std.ascii.isWhitespace(b)) break;
            if (idx < buf.len) {
                buf[idx] = b;
                idx += 1;
            }
            _ = nextByte();
        }
        const tok = buf[0..idx];
        if (std.mem.eql(u8, tok, "true")) return true;
        if (std.mem.eql(u8, tok, "false")) return false;
        abort.runtimeAbort("lo_read_bool: invalid token", 112);
    }
}

/// Read up to the next newline (consumed but excluded) into a fresh
/// `StringObject`. Returns the empty string on immediate EOF; use `lo_eof` to
/// disambiguate.
pub export fn lo_read_string() ?*Object {
    if (comptime is_wasm) {
        const raw = host_read_line_len();
        const len: u32 = if (raw < 0) 0 else @intCast(raw);
        const o = alloc.bumpAllocString(len);
        if (len > 0) {
            const data: [*]u8 = @ptrFromInt(@intFromPtr(o) + object.stringDataOffset());
            _ = host_read_line_into(data, @intCast(len));
        }
        return o;
    } else {
        var list = std.ArrayList(u8).init(std.heap.page_allocator);
        defer list.deinit();
        while (nextByte()) |b| {
            if (b == '\n') break;
            list.append(b) catch break;
        }
        const len: u32 = @intCast(list.items.len);
        const o = alloc.bumpAllocString(len);
        if (len > 0) {
            const data: [*]u8 = @ptrFromInt(@intFromPtr(o) + object.stringDataOffset());
            @memcpy(data[0..len], list.items);
        }
        return o;
    }
}

/// Return `true` iff stdin is at end-of-input, without consuming any bytes.
pub export fn lo_eof() bool {
    if (comptime is_wasm) {
        return host_eof() != 0;
    } else {
        return peekByte() == null;
    }
}
