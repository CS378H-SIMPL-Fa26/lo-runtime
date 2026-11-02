# Shared test corpus

Language-agnostic tests that live at the repo root because they apply to all
three skeletons equally. There are two distinct things here:

1. **LO program fixtures** (`lo_programs/` + `expected/`) — complete LO programs
   and their expected stdout. These exercise the runtime *through a compiled LO
   program*, so they are **not auto-runnable until a student's LO compiler
   exists**. For now they document the directory shape and the eventual
   conformance contract.
2. **The C harness** (`c_harness/`) — what actually verifies the skeletons
   *today*, at the ABI level, from C. (Each skeleton additionally has its own
   language-native unit tests under `rust/tests`, `zig build test`, and
   `cpp/tests` — those verify each skeleton in isolation.)

## LO program fixtures

| Program | Exercises | Expected |
|---|---|---|
| `lo_programs/alloc_basic.lo` | allocation + integer output | `expected/alloc_basic.out` (`42`) |
| `lo_programs/string_basic.lo` | String literal output | `expected/string_basic.out` (`hello`) |
| `lo_programs/class_basic.lo` | one class + method: alloc, dispatch, output | `expected/class_basic.out` (`7`) |

The eventual harness will, for each `lo_programs/<name>.lo`: compile it with the
team's LO compiler, link against the chosen skeleton's runtime, run it, and
compare stdout to `expected/<name>.out`.

> **Note:** the `.lo` surface syntax in these fixtures is validated against the
> redesigned LO-3 grammar (`planning:lo-3-reference.md`) — three-section class
> declarations, `int Main.main()` entry, the pre-bound `out` sink, and
> snake_case IO methods (`print_int` / `print_string` / `println`). The
> **expected outputs remain the stable contract**. The full conformance suite (a
> dozen-plus programs) is maintained separately in the `lo-testing` repo; this
> directory is the skeleton's small smoke set and may later mirror or merge with
> it (downstream decision).

## C harness — the live ABI check

`c_harness/main.c` is a **pure C** program that re-declares only the slice of the
C ABI it uses (it includes no skeleton headers) and links against a skeleton's
static library. If it links and runs, that skeleton honors the C ABI from C —
which is the entire promise the three skeletons make. It also `_Static_assert`s
the `Object`/`StringObject` layout from the C side, a fourth witness to the byte
layout.

Run it against every built skeleton:

```sh
# build the libs first:
( cd rust && cargo build )
( cd zig  && zig build )
( cd cpp  && cmake -B build -S . && cmake --build build )

bash tests/c_harness/run.sh
```

`run.sh` links the harness against each skeleton and checks all of them print
`42`. Linking notes it handles for you:

- **Rust** — linked with the native libs `rustc` reports
  (`cargo rustc -- --print native-static-libs`), so it works on Linux (needs
  `-lpthread -ldl -lm …`) and macOS.
- **Zig** — `std` uses raw syscalls; no extra libs beyond what `cc` links.
- **C++** — linked with the C++ driver (`c++`/`$CXX`) so the C++ standard library
  is pulled in.

## Cross-skeleton layout audit

The three skeletons are kept byte-identical against `runtime-abi.md` §2 by
compile-time checks that encode the *same numbers* in each language, plus the
C-side and empirical checks:

| Witness | Mechanism |
|---|---|
| Rust | `const _: () = { assert!(…) }` in `rust/src/object.rs` (pointer-width gated) |
| Zig | `comptime { std.debug.assert(…) }` in `zig/src/object.zig` |
| C++ | `static_assert(…)` in `cpp/include/lo_runtime/object.h` |
| C | `_Static_assert(…)` in `tests/c_harness/main.c` |
| Empirical | `run.sh` links C against each `.a` and confirms identical output |

All pin, on 64-bit: `sizeof(Object) == 16`, string data at offset `20`, shadow
frame roots at offset `16` (and `12 / 16 / 8` on 32-bit / WASM). If a skeleton's
layout ever drifts, its own compile-time check fails the build.

## Generated string GC stress corpus

`gc_metalang/` contains 20 `.lo_gc` workloads. The OCaml
[GC test generator](../gc-test-generator/README.md) lowers these to
`lo_programs/gc_meta_*.lo` and `expected/gc_meta_*.expected.out`.
Eighteen cases expect successful content preservation and two expect a diagnosed
OOM abort. Unlike the older smoke fixtures above, their stdout filenames end in
`.expected.out`. See the generator README for regeneration, semantics checks,
and WASM/native execution commands.
