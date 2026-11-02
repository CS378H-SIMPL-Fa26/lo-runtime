# String-allocation testing meta-language

Status: proposed v3; specification only. No generator is implemented.

## Purpose and scope

Make it easy to generate LO memory-allocation stress tests that check string
preservation under GC on WASM and x86-64. Describe payload sizes, assignments,
and function lifetimes, then generate ordinary LO-3 and independent expected
output. Keep the author-facing language small.

All workload values are strings or logical null. There are no object allocations,
fields, graph edges, cycles, class declarations, inheritance, or constructors in
this language. Test those features manually in LO. There are also no loops,
branches, recursion, return values, globals, or arbitrary string expressions.
The host generator expands repetition and seeded workload choices.

There is exactly one allocation command: `alloc N a`. There is no separate
`alloc_string`, `alloc_c`, `alloc_raw`, or `clear` command.

## Grammar

One statement or declaration per line. Blank lines and `#` comments are ignored.
Identifiers match `[A-Za-z_][A-Za-z0-9_]*`; keywords are reserved.
Names matching exactly `t[0-9]+` are compile-time temporaries; all other
non-parameter names are ordinary mutable locals. `tiny` and `temp` are ordinary
locals. Leading zeros are allowed: `t01` and `t1` are distinct names. Integers are
unsigned decimal literals representable as nonnegative LO integers.

```ebnf
program   = "heap" positive newline
            "expect" ("success" | "oom") newline
            function { function } ;
function  = "func" identifier natural newline
            { statement newline } "end" newline ;
statement = "alloc" natural binding
          | binding "=" value
          | "call" identifier ;
binding   = identifier ; (* t[0-9]+ has the static restrictions below *)
value     = identifier | "null" | empty ;
empty     = '""' ;
natural   = digit { digit } ;
positive  = natural ; (* greater than zero *)
```

The final newline is optional. Exactly one `func main 0` is required. Function
names are unique; forward calls are allowed. `func churn 2` has implicit parameters
`arg0` and `arg1`. Names matching `arg` followed by digits are reserved for declared
parameters. The call graph must be acyclic.

## Allocation and assignment

For an ordinary destination (not `t[0-9]+`), `alloc N a` constructs an N-byte ASCII string and assigns it to a, introducing the
binding if needed. For N > 0, generate a dynamic string operation such as repetition
of a deterministic one-character ASCII literal N times. Choose the character
reproducibly per source allocation site and record it in generation metadata.
Different sites may share a character; this is not an identity encoding.

N means payload bytes, not total heap bytes. A positive ordinary-variable allocation must remain a
runtime construction even if its content is known to the generator. Do not replace
this statement with a literal as an optimization. Reexecuting a site performs the
dynamic construction again. Evaluate the new value before overwriting the old
binding; preserve the old root until the assignment completes.

`alloc 0 a` assigns the canonical empty string and requests no fresh managed
string allocation. It still receives a logical allocation sequence number for
argument selection. Its implementation may use a generated helper's default
String field to obtain `LO_EMPTY_STRING`; account for helper setup separately.

Assignments have these meanings:

| Statement | Meaning |
| --- | --- |
| `a = b` | Copy an ordinary b by reference; if b is a compile-time temporary, materialize its literal into a. |
| `a = null` | Remove a's reference and make a ineligible as a call argument. |
| `a = ""` | Bind a to the canonical empty string; it is eligible as an argument. |

For ordinary names, an assignment can introduce a local. Temporary destinations
obey the stricter rules below. Reading an unintroduced binding is a
meta-compilation error. Reading a binding containing null is allowed: copying it
produces null. All locals have function scope, but hoisted LO declarations do not
make a binding logically introduced before its first assignment. Parameters are
introduced at entry. Every invocation has fresh locals.

Compile-time temporaries are not runtime roots. The following lifetime rules apply
to materialized strings in ordinary bindings and parameters.

Strings are immutable. Assignment changes a binding, not the referenced string.
Dropping one alias does not release a string retained by another binding. A string
can become collectible only after all active frames release their references.
Leaving a function releases that frame's bindings, not bindings in its callers.

## Calls: select the latest allocated strings

`call f` selects k strings, where k is f's declared arity, from the current frame.
It does not search enclosing caller frames or remember dead allocations.

Use the following deterministic rules:

1. Each executed `alloc` gets a monotonically increasing logical sequence number.
   An explicit `a = ""` also gets a fresh sequence number as an empty-value
   introduction, despite requiring no managed allocation.
2. `a = b` preserves b's sequence number. Parameter passing preserves the selected
   sequence numbers. Copying a value does not make its allocation newer.
3. Gather non-null values currently held in this frame, including compile-time
   temporary definitions, deduplicating aliases by
   logical sequence number. Equal-content strings from distinct allocation events
   remain distinct candidates. Empty introductions are likewise distinct logical
   events even though they share static storage.
4. Sort candidates by descending sequence number. The newest becomes `arg0`,
   the next newest `arg1`, and so on. Fewer than k candidates is an error.
5. Where multiple bindings hold the same candidate, choose its representative in
   parameter order, then local first-introduction order. This also determines
   whether the argument is a literal (temporary representative) or a variable
   (ordinary representative). No unchanged-binding heuristic is used.

Thus `alloc 10 a; alloc 20 b; c = a` selects b then a for a two-argument call;
c does not count as a third allocation. These semicolons are explanatory only;
actual source uses one statement per line.

Calls copy argument values. Reassigning `arg0` does not reassign its caller's
binding. Callers retain their ordinary runtime bindings during calls, including unpassed
ones. Compile-time temporaries occupy no caller root slots.
Functions have no result value. Null parameters cannot be selected, but a callee
may subsequently null a parameter.

## Compile-time temporaries: `t[0-9]+`

`alloc N t0` records the known N-byte content in the generator and a logical
sequence number for call selection. It emits **no LO declaration, variable,
assignment, or standalone allocation**. At an actual use, substitute the content
as an LO string literal. If never used, it emits nothing. This is deliberately
different from `alloc N ordinary`, which constructs a dynamic string.

Every temporary may be assigned once per invocation. Initialization with `alloc`,
`= another_binding`, or `= ""` is allowed; its value must be non-null and known
by symbolic execution. Copying an ordinary binding snapshots its content, not its
runtime identity. Later changes to the source do not change the temporary.
`= null` is invalid for a temporary. Any second assignment is an error, including
self-assignment, another `alloc`, or reassignment to equal content. The assignment
rule also rejects two definitions on one execution path; the language has no branches.

Ordinary variables remain variables even when assigned once. Parameters `arg0`,
`arg1`, etc. are ordinary runtime String parameters, even if the caller supplied
a literal. Their later calls use variables, not propagated compile-time substitutes.

Example:

```text
alloc 12 ordinary
alloc 4 t0
call churn
```

With x as the chosen payload character, lower the statements to:

```java
ordinary = ("x" * 12);
worker.churn("xxxx", ordinary);
```

There is no `t0` in the generated LO. The actual callee parameters are real runtime
bindings and must survive allocations in the callee.

The earlier unchanged-binding heuristic and variable fallback are removed.
`literal_threshold` is now an optional source-size guard (default 256 ASCII
characters, inclusive), not an LO limit. If a temporary's content exceeds it,
meta-compilation fails with a message to raise/disable the guard or use an ordinary
name. Never silently introduce a runtime temporary as a fallback. Setting the
guard to unlimited permits arbitrary supported source sizes, subject to resources.

A literal may be static on native and dynamically materialized on WASM. Reusing
a temporary may evaluate the same literal multiple times; the model must not
assume physical aliasing or zero WASM allocation. Count ordinary dynamic requests
separately from literal definitions and literal evaluations. `alloc 0 t0` substitutes
`""`; it does not promise canonical static identity at the use site.

## LO lowering and observations

Generate ordinary LO-3 methods with `Main.main()` as entry. Generated helper
classes/receivers needed by LO are implementation details; users do not describe
class layouts or fields. Helpers must not keep hidden references to old strings.

LO String is not nullable. Represent logical null using generator-tracked state
(or a boolean if needed) and overwrite the underlying LO String slot with the
canonical empty value to release the old dynamic reference. Null and empty remain
distinct logical states for argument selection and expected observations.
Do not emit illegal LO `String = null` assignments.

Compute expected output with an independent host-side model, not by running the
collector under test. For success programs, generate deterministic printed
checkpoint numbers and content-check results at function entry, before calls,
after calls, and before function exit. Observe active non-null runtime bindings in
the current frame, including caller bindings after a callee returns. Do not
materialize unused compile-time temporaries solely to check them. Check string
length and content using supported LO operations. Checkpoint density is a generator option: the exhaustive profile uses these
boundaries and optional statement checks; compact examples below use selected
phase checks and check each pressure allocation without printing on success.
Record the observation profile; it is not language syntax.

Discard observation temporaries before the next workload allocation. Avoid a
runtime registry of every allocation: it would prevent reclamation. Reference
strings constructed by checking code may allocate too; account for this overhead
and do not treat instrumented correctness timings as allocator throughput.
A final checkpoint alone cannot check already overwritten temporary strings;
use phase boundaries or additional checkpoints for that coverage.

## Size, alignment, and target behavior

The program describes the same requested payloads and logical binding operations
on both targets. It does not promise identical allocator traffic, live physical
bytes, collection timing, or the allocation at which OOM occurs.

The LO reference section 6.2 specifies a 16-byte object header on x86-64 and a
12-byte header on WASM. Strings add a 4-byte length before their inline payload.
Our Rust allocator currently rounds each allocation independently to 8 bytes:

| Positive string with N payload bytes | Managed allocation size |
| --- | --- |
| x86-64 | `roundUp8(20 + N)` |
| WASM | `roundUp8(16 + N)` |

These formulas describe the string allocation itself, excluding helper objects,
argument literals, root storage, checking temporaries, and GC side metadata.
Alignment is an allocator implementation choice here, not a meta-language command.
Do not hard-code the formulas as universal requirements on other LO collectors.

References: [LO reference](../LO-language-reference.pdf),
[allocation granules](rust/src/layout.rs), [string allocation](rust/src/alloc.rs).

## Heap configuration and contract outputs

`heap H` emits `// gc stress heap: H`. The P3 contract runs stress programs with
WASM's default heap, native's default heap, and native with `LO_HEAP_SIZE=H` set
before initialization. The host environment variable does not configure WASM's
heap. Record actual heap configuration per target.

For `expect success`, emit exit status 0 and a nonempty `.expected.out`. Keep
cumulative traffic large enough to pressure default heaps as well as small native
heaps. Allow space for live strings and temporary/setup allocations.

For `expect oom`, emit the runtime-abort metadata for exit 137 and stderr substring
`lo_alloc: out of memory`. Suppress checkpoints so expected stdout is empty even
when failure occurs at different allocations. Normal completion is a test failure.
Use an oversized allocation or retain enough strings to exhaust every configured
heap. An OS kill without the runtime diagnostic is not a passing OOM test.

Produce `.lo`, `.expected.out`, and generation metadata containing source, seed,
literal threshold, payload traffic, and target assumptions. Generated submissions
belong under `liveoak-contributed-tests/p3/LO-3/ValidPrograms` or
`RuntimeAbortPrograms`. This spec and the future generator live in `lo-runtime`.

Contract: [P3 grading contract](https://github.com/CS378H-Master/lo-testing/blob/main/grading-contract/p3-grading-contract.sh).
Local runner: [contributed test instructions](../liveoak-contributed-tests/p3/README.md).

## Paired examples and validation

[Example programs and LO-3 lowerings](examples/gc-metalang/README.md) include:

- `01_literal_alias`: literals, aliases after overwriting the original, caller-only
  roots, and 64 MiB of ordinary allocation pressure.
- `02_empty_scope`: empty versus null selection, variable argument preservation,
  function-local reclamation, and 64 MiB of pressure.
- `03_oversized_oom`: a 32 MiB request that must safely abort with a 16 MiB heap.
- `04_retained_oom`: five live 4 MiB strings that must exhaust a 16 MiB heap.

The files are illustrative manual lowerings, not evidence of an implemented
meta-compiler. The README records their generation conventions and validation.

## Workload guidance

The following literature informs workload patterns; this language does not run
the original benchmarks unchanged.

- [Wilson et al., allocator survey](https://www.cs.hmc.edu/~oneill/gc-library/Wilson-Alloc-Survey-1995.pdf):
  preserve correlations between sizes, lifetimes, and phases rather than relying
  solely on independent random sizes and deaths.
- [ACDC](https://acdc.cs.uni-salzburg.at/): adapt periodic churn around persistent
  memory using string bindings retained across function phases. Omit concurrency.
- [mimalloc-bench](https://github.com/daanx/mimalloc-bench#current-benchmarks): adapt
  alloc-test size distributions, sh6bench release orders, malloc-large requests,
  and mstress phase survivors. Nulling the last reference permits later GC; it
  does not reproduce the timing of explicit free.

Recommended generated families: overwrite churn with retained aliases; batch
allocation followed by forward/reverse release; persistent anchors interleaved
with temporary strings; grow/drop/regrow; sizes around alignment boundaries;
large requests after churn; caller-only roots during allocating callees; mixed
compile-time literal/variable arguments; and retained-string OOM.

Record requested payload traffic, peak logically retained payload, actual heap,
checkpoint results, and exit status. Logical payload is not total managed memory,
especially with literal arguments. Instrumentation is needed to measure actual
heap occupancy and collection counts. Keep timing runs separate from extensive
correctness checks and report compilation time separately for large generated
sources.

Tree benchmarks such as GCBench, cyclic graphs, shared object fields, and mixed
scalar/pointer layouts now require handwritten LO tests. They are intentionally
outside the strings-only generator's coverage.
