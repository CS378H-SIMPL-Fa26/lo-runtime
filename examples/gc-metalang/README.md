# String-only GC meta-language examples

Each `.gc` file is a complete meta-program. Its `.lo` file is a manually written,
ordinary LO-3 lowering, with an independent `.expected.out`. No meta-compiler is
implemented. These examples accompany [the specification](../../gc-testing-metalang.md).
They live here as executable documentation; they have not been submitted to the
contributed-test repository.

| Case | Meta-program | Lowering | Expected behavior |
| --- | --- | --- | --- |
| Literal, aliases, caller roots | [01_literal_alias.gc](01_literal_alias.gc) | [01_literal_alias.lo](01_literal_alias.lo) | Exit 0; five `true` lines. |
| Empty, null, frame lifetimes | [02_empty_scope.gc](02_empty_scope.gc) | [02_empty_scope.lo](02_empty_scope.lo) | Exit 0; four `true` lines. |
| Oversized request | [03_oversized_oom.gc](03_oversized_oom.gc) | [03_oversized_oom.lo](03_oversized_oom.lo) | Exit 137; empty stdout; runtime OOM diagnostic. |
| Retained allocations | [04_retained_oom.gc](04_retained_oom.gc) | [04_retained_oom.lo](04_retained_oom.lo) | Exit 137; empty stdout; runtime OOM diagnostic. |

## Lowering conventions

All allocation sites use x as their deterministic ASCII payload character. The
literal-size guard is 256. Generated methods are on Main, so calls use `this`;
no user-visible object semantics are added to the meta-language. Main's default
String field supplies the canonical empty string. The second example calls this
field `canonical` to avoid colliding with the meta-variable `empty`.

The examples use selected phase observations. Every pressure allocation is also
compared against an independently constructed expected string. A failed comparison
prints `false`, making stdout differ from the fixture. Successful pressure checks
are silent. This keeps output small without making pressure strings unobservable.

`phase4` calls `phase3` eight times, continuing down through `phase1` to `leaf`.
There are 8^4 = 4,096 leaf invocations, each requesting 16,384 bytes: **64 MiB of
workload payload**. Verification constructs another 64 MiB. This is finite acyclic
source, with no hidden repeat syntax, loops, or recursive meta-functions.
The leaf clears its local and returns; caller roots survive all phases.

## 01: literal versus variable arguments, alias preservation

The decisive source is:

```text
alloc 8 older
alloc 12 ordinary
alias = ordinary
alloc 20 ordinary
alloc 4 t0
call churn
```

The selected candidates are the t0 definition, then the 20-byte ordinary allocation.
The 12-byte allocation survives through alias; older is a separate unpassed root.
The lowering is:

```java
older = ("x" * 8);
ordinary = ("x" * 12);
alias = ordinary;
ordinary = ("x" * 20);
this.churn("xxxx", ordinary);
```

There is no declaration, assignment, allocation, or reference to t0 in the LO.
The callee checks both arguments after pressure; the caller checks older, alias,
and ordinary after return. Rebinding arg0 to empty does not change the caller.

## 02: null is not empty

After allocating anchor and discarded, the program nulls discarded and introduces
empty with `empty = ""`. The call selects empty then anchor, emitting
`this.hold(empty, anchor)`. Empty is an ordinary runtime binding, not a t-temporary.
The discarded allocation is excluded. Resetting callee arg0 to logical null clears
its String slot; the caller's empty binding remains active. Checks after pressure
verify the empty value and 1 KiB anchor in both frames.

## 03 and 04: safe aborts, not arbitrary crashes

03 requests one 32 MiB dynamic string. 04 retains five separate 4 MiB strings,
observing all five after the last allocation if execution gets that far. This
prevents the intended live-set demand from degenerating into dead-variable churn.

Both cases require the runtime's `lo_alloc: out of memory` stderr text in addition
to exit 137. A timeout, segmentation fault, or OS kill without that message fails.
Any normal completion fails, regardless of output. On a 64 KiB heap, the first
large request already fails; on a 16 MiB heap, 04 tests accumulated retention.
These OOM expectations assume a maximum tested default heap of 16 MiB. If the
runtime heap is increased, scale the requests rather than assuming these tests
must still abort. No negative-size allocation is expressible in this language;
other runtime-abort categories remain handwritten tests.

## Meta-compilation failures (not collector aborts)

These are specification examples of rejected programs, not runtime test cases:

```text
alloc 4 t0
alloc 8 t0       # second definition of a compile-time temporary
```

```text
alloc 4 t0
t0 = null       # temporaries cannot be cleared/reassigned
```

```text
alloc 257 t1    # exceeds the default 256-character literal guard
```

```text
alloc 4 a
b = a
call two        # if two has arity 2: only one distinct allocation exists
```

Raising/disabling the guard permits the third example. It must never trigger a
silent fallback to a hidden LO variable.

## Validation

Validation details are recorded below after running the examples. Compilation
checks the LO syntax/types; runtime checks compare exact stdout and exit status,
and require the diagnostic for OOM. Tests use the standard release collector,
not a verifier-enabled runtime. Expected output is specified above, not captured
from the implementation under test.

On 2026-11-02, all four LO files passed the local compiler's `check` pipeline.
All **12 runtime executions passed**: four WASM/default runs, four Linux x86-64
/default runs, and four Linux x86-64 runs with `LO_HEAP_SIZE=65536`.
Native execution used the isolated `colima-gc-validation` Rosetta environment on
Apple Silicon. Both runtime archives were built from this branch's Rust sources
(base commit `75949d7`). Success cases matched every expected output byte; OOM
cases exited 137 with empty stdout and the required diagnostic. This validates
these manual lowerings, not an unimplemented generator or the entire test suite.
