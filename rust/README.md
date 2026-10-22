# LO Runtime — Rust skeleton

Rust implementation of the LO runtime ABI ([`../runtime-abi.md`](../runtime-abi.md)).
The provided entry points work out of the box; the stubbed ones (`lo_gc_collect`,
the `lo_string_*` ops, `lo_cast_check` / `lo_instanceof`) panic with
`unimplemented!()` until your team fills them in for P3.

## Toolchain

- Rust stable (rustup). Built and tested against `cargo 1.87`.
- WASM target: `rustup target add wasm32-unknown-unknown`.

No external crate dependencies — the runtime is hand-rolled and native I/O goes
through `std::io`.

## Build

```sh
cargo build                                    # native staticlib + cdylib + rlib
cargo build --target wasm32-unknown-unknown    # WASM module
cargo build --release                          # LTO'd release
```

Native artifacts land in `target/debug/` (or `target/release/`):
`liblo_runtime.a` (static, for AOT linkage) and `liblo_runtime.{dylib,so}`. The
WASM build produces `lo_runtime.wasm` under `target/wasm32-unknown-unknown/`.

## Test

```sh
cargo test                                     # lib unit tests + ABI smoke tests
cargo clippy --all-targets -- -D warnings      # lints (must be silent)
cargo fmt --check                              # formatting
```

`cargo test` runs the compile-time layout locks, the `abi_smoke` suite (alloc
zeroing, shadow-stack push/pop, the `LO_EMPTY_STRING` singleton, bump-pointer
advance), and an I/O round-trip that drives the print/read entry points through a
child process (`src/bin/lo_io_probe.rs`).

## Layout

```
src/
├── lib.rs            # module wiring + re-exported C-ABI surface
├── object.rs         # ABI-visible types + layout locks
├── descriptors.rs    # LO_STRING_CLASS, boxed-primitive descriptors, LO_EMPTY_STRING
├── alloc.rs          # bump allocator behind lo_alloc
├── shadow_stack.rs   # lo_push_frame / lo_pop_frame
├── init.rs           # lo_runtime_init / lo_runtime_shutdown
├── io.rs             # lo_print_* / lo_read_* / lo_println / lo_eof
├── gc.rs             # lo_gc_write_barrier + the sliding mark-compact collector
├── gc/               # the collector's mark bitmap and offset table
├── layout.rs         # object sizes in granules, layouts, reference slots
├── string_ops.rs     # string op stubs
├── cast.rs           # cast/instanceof stubs
├── abort.rs          # runtime abort paths + lo_abort_null_receiver
└── bin/lo_io_probe.rs # I/O test driver (not part of the shipped runtime)
```

## What to implement in P3

Start in `gc.rs` (`lo_gc_collect` + the OOM-retry path in `alloc.rs`), then the
`string_ops.rs` and `cast.rs` stubs. The internal variable-size string allocator
`alloc::bump_alloc_string` is the pattern to build string results with. The
instructor's acceptance baseline is Cheney's semispace with all shared tests
passing (`../runtime-abi.md` §4.5).
