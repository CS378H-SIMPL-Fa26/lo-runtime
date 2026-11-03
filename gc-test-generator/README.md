# GC test generator

Compile a small strings-only workload into ordinary LO-3 plus independent expected
stdout. The compiler is OCaml; Python is used only for regression checks and the
optional execution runner. The [design document](design.md) defines the grammar
and semantics. No runtime or LiveOak compiler changes are required.

## Layout

Paths below are relative to `lo-runtime`:

- `gc-test-generator/compiler.ml`: parser, semantic model, LO emitter, CLI.
- `gc-test-generator/design.md`: design and language specification.
- `tests/gc_metalang/*.lo_gc`: 20 editable workload sources.
- `tests/lo_programs/gc_meta_*.lo`: generated LO-3 programs.
- `tests/expected/gc_meta_*.expected.out`: generated expected stdout.

The old manually lowered `examples/gc-metalang` directory is not used or recreated.

## Build and compile one file

The following commands run from `lo-runtime`. Requires OCaml (tested with 5.4.0)
and Make. Only the bundled Str library is used; no additional OCaml packages or
network downloads are needed. If OCaml is already on PATH, omit `opam exec --`.

```sh
opam exec -- make -C gc-test-generator

gc-test-generator/gc-metalang tests/gc_metalang/gc_meta_17_literal_alias_empty.lo_gc \
  -o tests/lo_programs/gc_meta_17_literal_alias_empty.lo \
  --expected tests/expected/gc_meta_17_literal_alias_empty.expected.out
```

For an arbitrary input path:

```sh
gc-test-generator/gc-metalang /path/to/workload.lo_gc
```

This writes `/path/to/workload.lo` and `/path/to/workload.expected.out`. Use `-o`
and `--expected` for separate destinations. Output directories must already exist.
Only `.lo_gc` input and `.lo` output suffixes are accepted. Existing outputs are
replaced after successful semantic validation; diagnostics go to stderr and errors
exit 1. A successful generation exits 0 without printing to stdout.

Regenerate all 20 cases and check generator semantics/reproducibility:

```sh
opam exec -- make -C gc-test-generator corpus
opam exec -- make -C gc-test-generator check
```

`make check` requires Python 3. It does not run the garbage collector.
`make clean` removes only the local OCaml executable and compiler intermediates.

## Language quick reference

```text
heap 65536
expect success

func consume 2
    alloc 4096 scratch
    scratch = null
end

func main 0
    alloc 12 ordinary
    alloc 4 t0
    call consume
    ordinary = ""
end
```

- Ordinary `alloc N name` dynamically constructs N ASCII payload bytes.
- Only names matching exactly `t[0-9]+` are compile-time constants. They are
  assigned once, have no generated runtime variable/allocation, and substitute
  literals at use sites. `tiny` and `temp` are ordinary mutable names.
- Each allocation site's character is a..z cyclically in **source order**. In the
  example, scratch uses a, ordinary b, and t0 c. The call therefore lowers to an
  ordinary-variable argument and the inline literal `"cccc"`, with **the literal
  first** because it is the newer definition.
- Calls select distinct live allocation events, newest first, in the current
  frame. Aliases do not count twice. Parameters are named `arg0`, `arg1`, etc.
- `a = b` copies a value; `a = null` drops a logical reference; `a = ""` binds an
  active empty string. Null and empty differ in argument selection.
- `alloc 0 a` uses canonical empty storage. Null is lowered by clearing the LO
  String slot to canonical empty; illegal LO String-to-null assignments are never
  emitted.
- No loops, recursion, field accesses, or objects. Use repeated acyclic calls for
  pressure. Object graphs and other runtime errors remain handwritten LO tests.

Compile-time literals default to at most 256 characters. To change this:

```sh
gc-test-generator/gc-metalang /path/to/workload.lo_gc --literal-limit 1024
```

`--literal-limit unlimited` removes the policy limit, subject to generator resource
limits. Oversized temporary definitions fail; they never silently become runtime
variables. Ordinary allocations are unaffected by this literal limit.

## Workloads

| Cases | Controlled experiment | Outcome |
| --- | --- | --- |
| 01–04: `live_004/016/064/128` | 4, 16, 64, 128 retained 512-byte strings; identical churn | success |
| 05–07: `count_256/032/004` | Fixed 64 KiB payload in 256 x 256 B, 32 x 2 KiB, or 4 x 16 KiB strings | success |
| 08: `lifetime_staircase` | Short, medium, and run-long retained values | success |
| 09–10: `replace_overlap/release` | Keep the old 8 KiB value during replacement versus explicitly release it first | success |
| 11–12: `holes_uniform/irregular` | Dead allocations before survivors, uniform versus irregular gap sizes, then a larger request | success |
| 13–14: `size_2047/2049` | Perturb case 06's 2048-byte payload by minus/plus one byte; same root schedule | success |
| 15–16: `frames_2/8` | Fixed 32 KiB spread over 2 or 8 suspended frames, followed by retirement and repetition | success |
| 17: `literal_alias_empty` | Literals, live aliases after overwrite, dropped roots, empty versus null | success |
| 18: `release_batches` | Release each 2 MiB helper frame before constructing the next of nine | success |
| 19: `retain_batches_oom` | Same nine requests, retain all values | OOM |
| 20: `oversized_after_churn_oom` | Bounded churn followed by one 32 MiB request | OOM |

There are **20 programs: 18 success and two expected OOM**. All filenames start
with `gc_meta_` and their two-digit case number. Every source includes its purpose.

`phase4` uses an acyclic eight-way call hierarchy to execute 4,096 leaf calls.
The ordinary leaf requests 16 KiB (64 MiB total); the replacement pair requests
8 KiB twice per leaf (also 64 MiB). Verification adds its own allocations.
Case 18 instead requests nine 2 MiB strings (18 MiB total), releasing each
helper frame before the next request. These totals pressure the default 16 MiB heap without assuming a
collector's metadata overhead. Some programs run multiple phases.

Large-root-set tests check the assigned value after each write and all active
roots at function/call boundaries. This avoids quadratic checking code that
overflows the current WASM host JIT's branch range. Releasing helper frames in
case 18 also retires compiler-rooted verification temporaries; merely nulling a
source binding need not retire those temporary roots.

The comparison pairs use the same assigned-value and function/call-boundary observation policy and
allocation-site content pattern. The replacement pair also has the same content
checks: nulling its only local adds no live-value checks. Object-count and frame
comparisons necessarily change root/check counts; their minimum passing heap
measures the entire instrumented program, not isolated object-header costs.
The release/retain pair compares outcomes, not timings: expected-abort programs
use silent guards instead of allocating content-check strings.

Successful baseline runs require exact stdout and exit 0. Expected-abort cases
require exit 137, empty stdout, and `lo_alloc: out of memory` on stderr. A crash,
timeout, OS kill without that diagnostic, or incorrect output is a failure.
The OOM demands assume a default heap of 16 MiB. Declared native budgets are
256 KiB except cases 18–19 (8 MiB, allowing a large value and its reference
strings in the success case). These budgets are baseline configurations, not
assertions of high pressure or universal minimum heap requirements.

## Run on WASM

Requires the LO compiler, Rust wasm32 target, LLVM clang, wasm-ld, and this
repository's wasmrun. Build from the parent workspace if needed:

```sh
(cd ../Liveoak && opam exec -- dune build bin/main.exe)
cargo build --manifest-path rust/Cargo.toml --release --lib --target wasm32-unknown-unknown
cargo build --manifest-path tools/wasmrun/Cargo.toml --release

python3 gc-test-generator/run_tests.py --target wasm \
  --compiler ../Liveoak/_build/default/bin/main.exe \
  --runtime-lib rust/target/wasm32-unknown-unknown/release/liblo_runtime.a \
  --wasmrun tools/wasmrun/target/release/wasmrun
```

On Homebrew installations you may need LLVM clang on PATH:
`export PATH="/opt/homebrew/opt/llvm/bin:/opt/homebrew/bin:$PATH"`.
The runner compiles/links all generated cases in a temporary directory and cleans
it afterward. Each subprocess has a timeout. WASM uses its default heap, not the
native `LO_HEAP_SIZE` environment override.

## Run on Linux x86-64

With the compiler built for the Linux host and the native Rust archive:

```sh
cargo build --manifest-path rust/Cargo.toml --release --lib
python3 gc-test-generator/run_tests.py --target native \
  --compiler ../Liveoak/_build/default/bin/main.exe \
  --runtime-lib rust/target/release/liblo_runtime.a
```

Each case runs twice: default heap, then the size in its `gc stress heap` header.
Do not pass a macOS or WASM archive as the native Linux archive.

For the existing isolated Docker/Rosetta environment on this Mac:

```sh
bash gc-test-generator/run_native_docker.sh
```

The script defaults to Docker context `colima-gc-validation` and image
`rust:1-bookworm`. It requires that environment to be running; it does not start
or reconfigure Docker/Colima. Override `DOCKER_CONTEXT`, `GC_DOCKER_IMAGE`, or
`LO_COMPILER` as needed. It emits assembly with the host compiler, builds the
current runtime in a temporary Linux amd64 container, then validates both heap
configurations. Host source is mounted read-only; scratch files and the container
are removed on exit. No output directories are added to the repository.

These commands use the standard release collector. A verifier-enabled runtime
also requires the compiler's static registration support; do not assume arbitrary
compiler/runtime feature combinations are interchangeable.

## Validation and limitations

The generator regression checks cover literal substitution, exact t<int> names,
argument order, alias deduplication, null/empty selection, temporary assignment
errors, literal limits, parameter forwarding, snapshots, multiple call contexts,
and reproducible generation of all 20 programs. These are tooling checks; the
runtime corpus is the deliverable that exercises the GC.

The corpus targets a single-threaded string workload. It does not cover object
fields, cycles, inheritance, or pointer identity. Expected OOM means a declared
runtime outcome, not a prediction made by the meta-compiler. The runner verifies
it. Full-content checking constructs reference strings and adds real allocation
traffic, so these are correctness stress tests, not uncontaminated throughput
benchmarks.

## Native heap sweeps

Keep WASM at its default heap. For native success programs, the runners can add
an explicit set of heap budgets after the mandatory default/header runs:

```sh
GC_CASE_GLOB='gc_meta_*replace*.lo' \
GC_SWEEP_HEAPS='32768 49152 65536 98304 131072' \
bash gc-test-generator/run_native_docker.sh
```

Or on Linux, add these options to the Python native-run command:

```sh
--pattern 'gc_meta_*replace*.lo' --sweep-heaps 32768,49152,65536,98304,131072
```

For the persistent-set ladder, use `gc_meta_0[1234]*.lo` as the pattern. The Python
runner accepts comma-separated sizes; Docker accepts space-separated sizes.
Both reject WASM heap reconfiguration (the Docker runner is native-only).

Each sweep trial is reported individually as correct completion or diagnosed
OOM. On OOM, any stdout already emitted must be a byte-for-byte prefix of the
expected output; an earlier incorrect check cannot be hidden by a later OOM.
The default and declared-heap success runs remain mandatory, so a collector
cannot pass by reporting OOM everywhere. Dedicated expected-abort cases retain
their strict baseline expectations and do not participate in these optional
success-workload sweeps.

Budgets are explicit byte values, not calculated from collector headers or side
metadata. Refine the chosen list around observed transitions if needed. These
runners do not assume monotonic outcomes, automatically binary-search a minimum,
or claim a collector-independent pass threshold. A passing low-budget run is
useful evidence; different correct collectors can require different budgets.

## Refactored corpus validation

On 2026-11-02, all 11 generator checks passed, including byte-for-byte corpus
regeneration. All 20 generated programs passed the local LO frontend and all
three baseline configurations: WASM default, native default, and native declared
heap (60 baseline executions). The 18 success programs matched exact expected
stdout; the two abort programs produced exit 137, empty stdout, and the required
runtime diagnostic.

Six success workloads were additionally swept at 32, 48, 64, 96, and 128 KiB:

| Workload | Diagnosed OOM at sampled budgets | Correct completion at sampled budgets |
| --- | --- | --- |
| 01: 4 anchors | 32 KiB | 48, 64, 96, 128 KiB |
| 02: 16 anchors | 32 KiB | 48, 64, 96, 128 KiB |
| 03: 64 anchors | 32, 48, 64 KiB | 96, 128 KiB |
| 04: 128 anchors | 32, 48, 64, 96 KiB | 128 KiB |
| 09: replacement overlap | 32 KiB | 48, 64, 96, 128 KiB |
| 10: release before replacement | 32 KiB | 48, 64, 96, 128 KiB |

All 30 sweep trials either completed correctly or reported diagnosed OOM with
stdout matching an expected-output prefix. The replacement pair is indistinguishable
at this coarse sampling resolution; no tighter threshold or improvement is claimed.
These measurements are for the current compiler/runtime and instrumented programs,
not universal GC requirements. Native execution used the existing isolated
Docker/Rosetta Linux x86-64 environment. No WASM heap reconfiguration was used.

WASM validation dependency: the recorded runs used the locally built wasmrun
containing commit `75949d7` (string-allocation OOM classification). Upstream main
at `d72873c` can classify OOM inside string repetition as exit 120 instead of the
required 137. That pre-existing runner fix is not included in this generator PR.
Use a runner with the correction to reproduce the passing WASM OOM results;
without it, the strict tests correctly fail. The native results do not depend on
that runner fix.
