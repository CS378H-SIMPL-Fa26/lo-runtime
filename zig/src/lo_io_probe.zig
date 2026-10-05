//! I/O round-trip driver, mirroring the Rust skeleton's probe. Reads one integer
//! from stdin and echoes it, then prints the literal 42, each on its own line.
//! Driven by the `test` build step (see build.zig). Not part of the shipped
//! runtime.

const rt = @import("lo_runtime.zig");

pub fn main() void {
    rt.init.lo_runtime_init();

    const n = rt.io.lo_read_int();
    rt.io.lo_print_int(n, 0);
    rt.io.lo_println(0);

    rt.io.lo_print_int(42, 0);
    rt.io.lo_println(0);

    rt.init.lo_runtime_shutdown();
}
