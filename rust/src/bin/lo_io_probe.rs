//! Tiny driver that exercises the I/O entry points through real stdin/stdout, so
//! the integration tests can round-trip them in a child process (no fd hackery,
//! no cross-test interference). Not part of the shipped runtime.
//!
//! Protocol: reads one integer from stdin and echoes it, then prints the literal
//! `42`, each on its own line. Run by `tests/io_roundtrip.rs`.

fn main() {
    lo_runtime::lo_runtime_init();

    let n = lo_runtime::lo_read_int();
    lo_runtime::lo_print_int(n, 0);
    lo_runtime::lo_println(0);

    lo_runtime::lo_print_int(42, 0);
    lo_runtime::lo_println(0);

    lo_runtime::lo_runtime_shutdown();
}
