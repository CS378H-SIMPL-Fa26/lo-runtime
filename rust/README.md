# LO Runtime — Rust

Rust implementation of the LO runtime ABI ([`../runtime-abi.md`](../runtime-abi.md)).
This branch implements sliding mark-compaction, string operations and runtime
type checks. The optional heap verifier checks the collector on both targets.

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
├── gc/               # mark bitmap, offset table and debug heap verifier
├── layout.rs         # object sizes in granules, layouts, reference slots
├── string_ops.rs     # string operations
├── cast.rs           # cast/instanceof checks
├── abort.rs          # runtime abort paths + lo_abort_null_receiver
└── bin/lo_io_probe.rs # I/O test driver (not part of the shipped runtime)
```

## Debug heap verifier

Build with `--features gc-verify` to check the heap **before and after every
collection**, including allocation-triggered collections. This also works in
release builds and on `wasm32-unknown-unknown`; it needs no environment variable.
Without the feature there are no registry, bitmap or verifier walks.

The verifier checks:

- bounded, aligned object traversal up to the allocation frontier, including
  variable-size strings and zero GC header bits;
- descriptor identity against descriptors recorded at the trusted allocation
  boundary, with snapshots of size and pointer offsets;
- every root and pointer field against **exact object starts**, null, or an
  explicitly registered immortal object (interior and free-tail pointers fail);
- registered statics' headers and edges (statics may not hold managed edges,
  because this collector does not trace them);
- `0xEF` in the entire free tail, including never-allocated space. Allocation
  zeroes each claimed region; collection re-poisons the full free tail.

The scratch object-start bitmap is allocated at heap initialization and reused.
Descriptor/static registration may allocate Rust-side metadata, but the successful
verification walk allocates nothing and never invokes the managed allocator.
It does not use the collector's mark/forwarding tables to validate object starts.
Failure reports `lo_gc_verify: before/after collection: ... at ...` and exits
134 on native or reports through host stderr and traps on WASM.

`LO_EMPTY_STRING` is registered automatically. Compilers/embedding programs must
call the diagnostic extension `lo_gc_register_static(object)` after runtime init
for every other immortal object, before storing it in a root or heap field.
The object and its descriptor/offset table must be valid immutable storage for
the runtime's lifetime. The extension is exported as a no-op when verification
is disabled, and never collects. Liveoak registers native literal objects and
both backends' preamble objects at startup. Raw descriptor/function addresses
belong in untraced values, not object roots. This extension requires the updated
runtime when linking the updated compiler; it does not change object/frame ABI.

Trusted boundaries: valid shadow-frame links/storage and the descriptor/static
storage supplied to the registration/allocation APIs. The checker cannot recover
a missing root or detect a stale address that happens to identify another valid
object. It is a structural diagnostic, not a replacement for graph/payload tests.

### Reproduce the checks

From `rust/`:

```sh
cargo test
cargo test --features gc-verify
cargo test --features gc-stress
cargo clippy --all-targets --features gc-verify -- -D warnings
cargo fmt --check
bash tests/gc/check.sh
```

The last command runs on a native host and WASM with `cc`, `clang`,
`wasm-ld`, `wasmrun` and the Rust WASM target installed. It builds both targets
with `gc-stress`, then runs the same ten original C-ABI probes on each: a preserving
cycle/shared-reference/string/static graph, eight deliberate corruptions, and
a live-set overflow that must still produce the ABI's out-of-memory abort.
Negative tests must produce the expected verifier diagnostic, not merely crash.

`gc-stress` implies `gc-verify` and changes the default heap to **4 KiB** for
deterministic pressure on both targets. Normal builds still default to 16 MiB.
Native `LO_HEAP_SIZE` continues to override either default; freestanding WASM
has no environment, so use the build feature for a tiny heap there. Do not use
the stress feature for programs whose legitimate live set exceeds 4 KiB.

Compiler-level header-contract tests are included under `tests/gc/programs/`;
see the commands below.

### Finding collector bugs

From the runtime repository root:

```sh
bash rust/tests/gc/check.sh
# Only the host-native probes (no WASM toolchain required):
MODES=native bash rust/tests/gc/check.sh
# Reproduce or extend a deterministic graph run:
SEEDS="1 7 42 12345 3735928559" bash rust/tests/gc/check.sh
# Install the official nightly toolchain once for sanitizer support:
rustup toolchain install nightly --profile minimal
# Instrument the Rust collector AND C probes:
bash rust/tests/gc/asan.sh
```

On macOS, use Homebrew LLVM rather than Apple Clang for WASM and ASan:

```sh
export PATH="$(brew --prefix llvm)/bin:$(brew --prefix lld)/bin:$PATH"
```

`check.sh` retains the ten original ABI cases, then adds:

- `boundaries.c`: objects beginning just before, at, and after a 512-byte
  bitmap-block boundary; string lengths around granule/block boundaries;
  overlapping moves; repeated hole creation; an exactly full 4 KiB heap;
  reclamation of that entire heap and repeated empty collections.
- `graph.c`: five reproducible seeds, each with 2,000 mutations of roots and
  edges, allocations, and forced collections. A separate integer-only model
  computes reachability. The test compares unique generation IDs, payloads,
  both edges, cycles and shared identity after each mutation and collection.
  Its temporary address map is rebuilt after every possible collection.
- `gc-every-alloc`: a separate opt-in feature, implying `gc-stress`, that
  collects **before every managed allocation**, including strings. The valid
  preservation case and graph seeds run again with this feature. Boundary and
  deliberate-corruption probes run only in the original mode because they
  depend on exact allocation placement or a specific collection point.

Native probes use the host architecture (ARM64 on an Apple Silicon Mac); run
in the course Linux x86-64 container to cover the compiler's native target.

The graph oracle catches wrong-but-valid references that the structural verifier
cannot distinguish. ASan instruments both the C harness and Rust runtime (the
prebuilt Rust standard library is not rebuilt). It catches host allocation
bounds/use-after-free errors, but does not know individual object boundaries
inside the managed heap; the verifier and model cover that gap. ASan failures
stop the script. `CC`, `WASM_CC`, `WASM_LD`, and `WASMRUN` select test tools;
`CC` also selects the ASan C compiler.

Compiler tests live in `rust/tests/gc/programs/`: recursive caller roots,
receivers held across allocating arguments, staged arguments, string fields,
and string expression temporaries. From a compatible Liveoak checkout in the
Linux x86-64 course environment, with this runtime checked out at `runtime/`:

```sh
cargo build --manifest-path runtime/rust/Cargo.toml --release --lib --features gc-every-alloc
cargo build --manifest-path runtime/rust/Cargo.toml --release --lib --features gc-every-alloc --target wasm32-unknown-unknown
LO_HEAP_SIZE=4096 dune exec -- lo_test_runner --tests runtime/rust/tests/gc/programs --level LO-3 --mode native,compile --no-docker
```

These require compiler support for static registration and untraced descriptor
addresses, as described above. Run the larger general conformance suite with
`gc-verify`, not a 4 KiB heap: some programs intentionally retain larger graphs.
