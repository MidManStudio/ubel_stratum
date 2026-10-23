# LLVM proof of concept

Status: **Stage 0 and Stage 1A measured on CI (four runs). Stage 1B (emit IR from Ubel source) has a first slice that matches the interpreter on a small subset, verified on CI on x86_64 and aarch64. A browser-playground feasibility check is recorded below.**

> **Everything in this document is provisional.** This is a proof of concept. The
> numbers come from a hand-written runtime on two shared CI runners, the designs
> (closure ABI, scannable arenas, copy-out, arena lifecycle) are proposals, and none
> of it has been compared against mature implementations yet. Memory-management
> and cross-tier questions like these need proper research and benchmarking before
> any of them becomes a language or compiler contract. See "Validation still to do".

Everything lives in `crates/llvm_poc/`. The C runtime and IR benchmarks depend on
nothing else. The emitter crate (`ubel_llvm`) is its own workspace root and depends on
the core and parser crates by path, read-only, so it cannot interfere with work in
`core`, `sema` or `rd_parser`.

## Why this exists

The interpreter cannot say anything about the memory tiers. It runs HIGH, MID and LOW
on the same reference-counted values, and `with arena` blocks are validated but never
allocate. Before building a real backend, the PoC measures what each tier costs as
native code, how the tiers behave at their boundaries, and whether Ubel source can be
lowered to IR that behaves like the interpreter.

| stage | what | status |
|---|---|---|
| 0 | the three tiers in isolation, hand-written IR | measured on CI (runs #1, #3, #4) |
| 1A | how the tiers talk to each other, hand-written IR | measured on CI (runs #3, #4) |
| 1B | emit IR from Ubel source, compare with the interpreter | first slice verified on CI (15 of 15 on both runners) |

## Stage 0: the three tiers in isolation

The same workloads are written three times in hand-written LLVM IR, once per tier, in
the shape a tier-aware backend would emit. Every tier must produce the same checksum.

| tier | allocation strategy in this PoC |
|---|---|
| HIGH | calls into a stop-the-world, non-moving mark-sweep GC (`runtime/ubel_rt.c`) |
| MID | arena create, inlined bump-pointer allocation, destroy |
| LOW | direct `malloc` / `free`, freed explicitly |

Workloads: `churn` (temporary lists that die every frame), `churn+live` (the same churn
while a 6.4 MB list stays live, which makes GC marking cost visible), `tree`
(AST-style build, walk, drop), `arith` (no allocation, a sanity baseline).

```
crates/llvm_poc/runtime/ubel_rt.h        API of the three allocation strategies and the print helpers
crates/llvm_poc/runtime/ubel_rt.c        GC (size-class spans, tri-colour mark-sweep) and arenas
crates/llvm_poc/runtime/ubel_main.c      main() for programs emitted by Stage 1B
crates/llvm_poc/bench/tiers.ll           Stage 0 workloads, hand-written LLVM IR
crates/llvm_poc/bench/boundaries.ll      Stage 1A: tier-to-tier calls and data, hand-written LLVM IR
crates/llvm_poc/harness/bench_main.c     timing, checksum checks, markdown report, --require-gc
crates/llvm_poc/scripts/run_bench.sh     build and run the benchmarks (also --selftest)
crates/llvm_poc/Cargo.toml, src/         Stage 1B: the emitter crate (ubel_llvm)
crates/llvm_poc/examples/*.ubl           Stage 1B test programs
crates/llvm_poc/scripts/run_equiv.sh     Stage 1B: native build against the interpreter
crates/llvm_poc/scripts/run_wasm_demo.sh Feasibility demo: emitted IR compiled to wasm32 and run in Node
crates/llvm_poc/wasm/wasm_rt.c, run.js   freestanding wasm runtime and the Node host for that demo
```

## Running it

```
bash crates/llvm_poc/scripts/run_bench.sh --selftest   # correctness first
bash crates/llvm_poc/scripts/run_bench.sh              # Stage 0 and 1A benchmarks
bash crates/llvm_poc/scripts/run_bench.sh --iters 9 --only tree
bash crates/llvm_poc/scripts/run_bench.sh --only xing  # Stage 1A only (xing-lifecycle, xing-callback, xing-iter, xing-low)
bash crates/llvm_poc/scripts/run_equiv.sh              # Stage 1B: emitter against the interpreter
bash crates/llvm_poc/scripts/run_wasm_demo.sh          # emitted IR as WebAssembly, run in Node (needs wasm32 clang, wasm-ld, node)
```

The benchmarks need clang 15 or newer (opaque pointers). `EXTRA_FLAGS=-m32` and
`CLANG=clang-18` are supported. The Stage 1B script also needs cargo, because the
emitter is a Rust crate. On Termux: `pkg install clang`.

**Run `--selftest` once on every new target before trusting its timings.** It forces a
collection every 16 KiB and fails unless all tiers agree, the collector really ran,
and it kept a known live list alive. The report also prints a note when HIGH did zero
collections.

On GitHub: the "Ubel LLVM PoC" workflow runs the self-test and the benchmarks on x86_64
and aarch64 runners and writes tables to the job summary. The optimized IR is uploaded
as an artifact.

## Results

### Run #1: GitHub runners, clang 18.1.3, 7 timed runs per tier, scale 1

Minimum time in ms (the median was within about 3% of the minimum everywhere). Ratio is
tier time divided by HIGH time, so below 1.00 is faster than HIGH.

| runner | benchmark | HIGH (GC) | MID (arena) | LOW (malloc/free) | MID / HIGH | LOW / HIGH |
|---|---|---:|---:|---:|---:|---:|
| x86_64 | churn | 48.70 | 10.45 | 58.99 | 0.21 | 1.21 |
| x86_64 | churn+live | 115.13 | 11.02 | 62.15 | 0.10 | 0.54 |
| x86_64 | tree | 47.01 | 17.36 | 72.53 | 0.37 | 1.54 |
| aarch64 | churn | 45.17 | 15.63 | 63.93 | 0.35 | 1.42 |
| aarch64 | churn+live | 98.42 | 16.56 | 67.49 | 0.17 | 0.69 |
| aarch64 | tree | 43.03 | 15.43 | 73.31 | 0.36 | 1.70 |

### Run #3: same runners, 15 timed runs per tier, scale 1

Commit 18fccb2, CI run number 3.

| runner | benchmark | HIGH (GC) | MID (arena) | LOW (malloc/free) | MID / HIGH | LOW / HIGH |
|---|---|---:|---:|---:|---:|---:|
| x86_64 | churn | 29.46 | 6.32 | 29.27 | 0.21 | 0.99 |
| x86_64 | churn+live | 75.59 | 6.65 | 31.28 | 0.09 | 0.41 |
| x86_64 | tree | 24.77 | 8.05 | 38.37 | 0.33 | 1.55 |
| aarch64 | churn | 44.82 | 15.63 | 63.83 | 0.35 | 1.42 |
| aarch64 | churn+live | 97.87 | 16.52 | 67.56 | 0.17 | 0.69 |
| aarch64 | tree | 42.92 | 15.55 | 72.90 | 0.36 | 1.70 |

GC activity in the fastest HIGH run of run #3:

| runner | benchmark | collections | GC time (ms) | share of run | max pause (ms) | live at last GC |
|---|---|---:|---:|---:|---:|---:|
| x86_64 | churn | 30 | 0.20 | 0.7% | 0.018 | 5 KiB |
| x86_64 | churn+live | 21 | 44.34 | 58.7% | 2.173 | 6265 KiB |
| x86_64 | tree | 24 | 0.08 | 0.3% | 0.005 | 7 KiB |
| aarch64 | churn | 30 | 0.24 | 0.5% | 0.014 | 5 KiB |
| aarch64 | churn+live | 21 | 50.79 | 51.9% | 2.481 | 6265 KiB |
| aarch64 | tree | 24 | 0.15 | 0.4% | 0.007 | 3 KiB |

### Run #4: same runners, 15 timed runs per tier, scale 1

Commit 22b0865, CI run number 4 of the benchmark workflow.

| runner | benchmark | HIGH (GC) | MID (arena) | LOW (malloc/free) | MID / HIGH | LOW / HIGH |
|---|---|---:|---:|---:|---:|---:|
| x86_64 | churn | 41.27 | 8.73 | 48.38 | 0.21 | 1.17 |
| x86_64 | churn+live | 99.08 | 9.36 | 51.49 | 0.09 | 0.52 |
| x86_64 | tree | 37.54 | 13.94 | 56.28 | 0.37 | 1.50 |
| aarch64 | churn | 44.82 | 15.62 | 63.99 | 0.35 | 1.43 |
| aarch64 | churn+live | 98.38 | 16.65 | 67.65 | 0.17 | 0.69 |
| aarch64 | tree | 41.80 | 14.83 | 72.06 | 0.35 | 1.72 |

GC activity in run #4, `churn+live`: 21 collections, 55.91 ms of GC (56.4% of the run) and
a 2.72 ms maximum pause on x86_64; 51.27 ms (52.1%) and 2.49 ms on aarch64.

**Runner variance, from three runs.** The aarch64 runner is stable: `churn` HIGH was 45.17,
44.82 and 44.82 ms across runs #1, #3 and #4. The x86_64 runner is not: the same workload
took 48.70, 29.46 and 41.27 ms, a 65% spread, so the x86 pool is probably not uniform
hardware (not verified; later reports print the CPU model so this can be checked). Ratios
mostly held, with one exception: `churn` LOW/HIGH on x86 was 1.21, 0.99 and 1.17, so the
parity seen in run #3 was an outlier on faster hardware. Compare tiers within one run, run
more than once, and treat small ratios as hardware-dependent.

### What the numbers say (both runs, both architectures)

- **MID is the clear win for bulk temporary data.** 2.7x to 11x faster than HIGH and
  4x to 6x faster than LOW on every workload. Allocation is a bump of about 1.5 to 5 ns
  and freeing the whole arena costs almost nothing.
- **HIGH versus LOW depends on how much data is live, and on the hardware.** With little
  live data (`churn`, `tree`) HIGH ranges from parity with `malloc`/`free` to 1.7x faster
  (most runs 1.2x or better): garbage costs almost nothing to collect (GC is under 1% of the run) while `free` pays
  per object. With a 6.4 MB live set (`churn+live`) the order flips and `malloc`/`free`
  is 1.4x to 2.4x faster than HIGH.
- **HIGH's cost has two parts.** The allocation path itself is about 7 to 14 ns per
  object against about 1.5 to 5 ns for a bump allocation (HIGH's figure moves with the x86
  hardware). On top of that, marking the
  live set costs about 10 to 15 ns per live list node on every collection, and with a
  6.4 MB live set that was 52% to 59% of the run in all four runs. Collections are paced by the live size
  (GOGC=100 style), so the cost scales with live data times the number of collections.
- **Pause.** One stop-the-world collection of the 6.4 MB live list took about 2.2 to
  3.1 ms. A linked list is the worst case for tracing (every step is a dependent load),
  so that is a pessimistic per-node figure, but it does grow with the live heap.

### What they do not say

- No concurrency. A concurrent collector would move marking off the mutator thread,
  shrinking pauses and wall time, at the price of write barriers that are not measured
  here. Stage 0 says nothing about Go-style pause behaviour.
- One thread, one object size (32 bytes), and a list-shaped live set. Trees and arrays
  mark differently.
- LOW is glibc `malloc`. A pool or a different allocator would change LOW.
- Roots are conservative and globals are not roots. The GC is a first cut and has not
  been tuned; its allocation fast path in particular has room to improve.
- Two shared CI runners with the variance noted above. Absolute times are not comparable
  between runners or between targets (a `Node` is 32 bytes on 64-bit and 28 on 32-bit).
- Hand-written IR models the tiers. It is not code generated from Ubel source, so it
  shows what the memory strategies could cost, not what the compiler produces.

## Stage 1A: how the tiers talk to each other

Stage 0 measured each tier alone. Stage 1A is about the boundaries: what the generated
code has to do when one tier calls another or hands it data. It is hand-written IR
(`bench/boundaries.ll`), modelled on `examples/arena_demo/ast_builder.ubl`, because that
real example needs strings, closures, enums with payloads and `match`, which an emitter
will not cover for a while. The IR is what an emitter would have to produce.

### How each direction of the call matrix lowers

| call | rule (tier-model.md) | what the IR does | what keeps it safe |
|---|---|---|---|
| HIGH to MID | allowed | a plain call; the MID function creates its own arena (`with arena` is create ... destroy, and destroy must run on every exit path, including `return`) | GC values passed in stay reachable from the caller's frame |
| HIGH to LOW | allowed | a plain call, pointers into GC objects passed as borrows | the collector never moves objects, and the caller's frame keeps them alive |
| MID to LOW | allowed | a plain call, arena memory passed as a pointer | same; LOW code allocates nothing and calls no other tier, so no collection can start while it runs |
| MID to HIGH | only through a function-typed parameter | one indirect call through the closure's code pointer | the closure object is a GC object held by the MID frame; arena pointers handed to HIGH are outside the GC heap and ignored by the collector |
| LOW to HIGH or MID | never | no code emitted | enforced by sema |

The calls themselves cost nothing extra in this PoC: every tier uses the same calling
convention and the same data layouts, and a reference's tier exists only in the type
checker. What a boundary costs is the setup around it: arena lifecycle, the indirect
call, GC allocation of results, and copying.

### Closure ABI (proposal)

A closure is a GC object whose first word is the code pointer and whose remaining words
are the captured variables. The code takes the closure object itself as a hidden first
argument: `ptr code(ptr closure, ptr argument)`. A callback is then one indirect call,
and captured state is reachable with no extra allocation. This is a decision the backend
still has to confirm against real Ubel closures (capture layout, generics, callbacks that
outlive their scope, statically known calls); the PoC only shows it works across the
boundary, including captured GC data, while collections run inside the callback.

### Safety across the boundary

- **GC pointers on MID and LOW frames are safe.** Stack and register scanning is
  conservative, so a GC pointer held by a MID or LOW frame keeps its object alive without
  any registration step.
- **The hidden-root hazard is real and reproduced on CI.** A plain arena is invisible to
  the collector. If its contents are the only reference to a GC object (the iterator
  pattern in tier-model.md keeps `List<R>` results, GC-owned, in MID-side storage), the
  collector frees live results. With a collection every 16 KiB the plain-arena variant
  returned a wrong answer on both runners, 64-bit and 32-bit builds alike. Two fixes
  work and are implemented: keep results in a GC-owned list, or make the arena
  **scannable** (`ubel_arena_create_scan`), which makes every collection treat its
  contents as a conservative root range.
- **The other direction is already guarded by sema.** The escape checks reject arena
  references returned, assigned to outer bindings, stored in struct fields or captured by
  closures. The hazard here is the opposite one, GC references stored inside arena
  memory, which those checks do not cover.
- **Open design questions, not decided:** whether arena-resident containers may hold GC
  references at all (the tier-model iterator pattern implies yes); whether the compiler
  should pick the arena kind statically (scannable when any type stored in it can hold a GC
  reference, plain otherwise); and whether root registration, handles or an ownership
  restriction would do better than scanning whole arenas. These are not mutually
  exclusive and need research before one is chosen.
- **Owned results are deep copies.** Data built in an arena that must outlive it
  (`to_owned()`) is copied into the GC heap. `xcb_copyout` does this and then reads the
  copy after the arena is destroyed and its chunk reused.
- **For a concurrent collector later:** every pointer store into a GC object (child links,
  closure creation, list links) is a write-barrier site, and a GC pointer stored into a
  scannable arena needs the same treatment.

### Results (run #3, 15 runs, commit 18fccb2)

**Arena lifecycle.** ns per object when a fixed 2,000,000 objects are allocated in groups
of N per arena. HIGH just allocates, MID creates, fills and destroys an arena per group,
LOW mallocs and frees.

| N | x86 HIGH | x86 MID | x86 LOW | arm HIGH | arm MID | arm LOW |
|---:|---:|---:|---:|---:|---:|---:|
| 1 | 6.8 | 7.5 | 5.2 | 10.5 | 16.0 | 11.5 |
| 2 | 6.8 | 4.2 | 5.0 | 10.6 | 9.1 | 11.5 |
| 4 | 6.8 | 2.5 | 5.0 | 10.8 | 6.7 | 12.0 |
| 8 | 6.8 | 1.9 | 5.5 | 11.1 | 5.6 | 13.8 |
| 16 | 7.2 | 1.6 | 6.2 | 11.1 | 4.7 | 15.2 |
| 32 | 7.2 | 1.6 | 7.1 | 11.2 | 4.4 | 15.8 |
| 64 | 7.3 | 1.5 | 7.2 | 11.2 | 4.2 | 15.9 |
| 256 | 7.3 | 1.7 | 7.3 | 11.2 | 4.0 | 15.9 |
| 1024 | 7.3 | 1.6 | 7.4 | 11.1 | 3.9 | 16.1 |

MID beats HIGH and LOW from 2 objects per arena on both architectures, in both runs. At N = 1
an arena costs more than just allocating (1.10x to 1.26x HIGH on x86, 1.52x to 1.57x on
aarch64). The fixed cost of one arena is roughly the N = 1 time minus the steady state:
6 to 11 ns on x86 (it moved between runs with the hardware) and 12 ns on aarch64. Run #4
measured 12.8, 6.5, 3.8, 2.7 and 2.4 ns per MID object at N = 1, 2, 4, 8 and 16 on x86,
with the same shape on aarch64.

**Callback and copy-out.** Per repetition, a binary expression tree is built and
evaluated. `callback`: HIGH calls MID, MID builds the tree in an arena and calls a HIGH
closure with a pointer into it; the closure evaluates it and returns a GC-owned result.
`copy-out`: the closure deep-copies the tree into the GC heap, the arena is destroyed,
and HIGH evaluates the copy. The other three do the whole job in one tier. ns per tree.

| runner | tree | callback | copy-out | MID only | HIGH only | LOW only |
|---|---|---:|---:|---:|---:|---:|
| x86_64 | depth 4 (31 nodes) | 91 | 284 | 81 | 223 | 281 |
| x86_64 | depth 9 (1023 nodes) | 2696 | 9070 | 2764 | 7516 | 9432 |
| aarch64 | depth 4 (31 nodes) | 155 | 469 | 142 | 381 | 535 |
| aarch64 | depth 9 (1023 nodes) | 4323 | 15785 | 4305 | 12780 | 18714 |

- **Crossing cost (callback minus MID only), over runs #3 and #4:** 10 ns at depth 4 on
  both architectures in both runs (13 ns once on aarch64), and 15 to 18 ns at depth 9 where
  it was positive. Depth 9 also produced -68 ns and -44 ns, which is noise: about +/-50 ns is
  the floor of this benchmark at that size, so the true crossing cost there is below what it
  can resolve. These figures include the closure's work (an indirect call, a counter update
  and a GC allocation of the result), not just the call.
- **The callback is cheap compared with doing the job in one tier:** 2.4x to 3.0x faster
  than building the tree in the GC heap.
- **Owned results cost real work.** Copy-out is 6 to 10 ns per node copied on x86 (it moved
  with the hardware) and 10.5 to 11.2 on aarch64, and arena build plus copy-out is 1.2x to
  1.3x slower than building straight in the GC heap in every run. If the whole result has to be GC-owned anyway, the
  arena buys nothing here; it pays when the callback reads and returns something small.
- The callback variant allocates only a 16-byte result per tree, so it never reached a
  collection at the default trigger. GC behaviour inside callbacks is exercised by
  copy-out, the iterator pattern and the self-test.

**Iterator pattern** (50,000 items, a HIGH closure turns each arena item into a GC-owned
result and allocates garbage, so collections happen while the MID frame is live):

| runner | results kept in | time (ms) | ns/item | GC time (ms) | max pause (ms) | correct |
|---|---|---:|---:|---:|---:|---|
| x86_64 | scannable arena array | 1.62 | 32 | 0.44 | 0.261 | yes |
| x86_64 | GC-owned list | 2.25 | 45 | 0.75 | 0.498 | yes |
| aarch64 | scannable arena array | 2.85 | 57 | 0.90 | 0.518 | yes |
| aarch64 | GC-owned list | 3.51 | 70 | 1.05 | 0.690 | yes |

The scannable arena array was 18% to 28% faster here than a GC-owned list across runs #3 and
#4 (run #4: 2.53 against 3.29 ms on x86, 2.86 against 3.50 ms on aarch64), because the list
costs an extra GC allocation per item. That is one workload with a small arena;
scanning cost grows with arena size, so it does not mean scannable arenas are the answer.

**Hazard demo** (20,000 items, a collection every 16 KiB of GC allocation), identical on
both runners in both runs: the plain arena array returned a **wrong** result (253 collections, live
results freed); the scannable arena returned the correct result (53 collections). The
collection counts differ because collections are paced by live size, and the plain arena
hides its live data from the collector.

**Calls into LOW** (32-word buffer sum): HIGH to LOW 2.99 ns, MID to LOW 3.07 ns, inlined
baseline 3.00 ns on x86 in run #3, and 4.19, 4.18 and 3.81 ns in run #4; 7.00, 7.01 and
6.82 ns on aarch64. The call boundary costs between 0 and 0.4 ns, which is not measurable
against a few ns of work, but this is a small function with no allocation, so it
says nothing yet about larger functions, indirect calls or cross-module optimization.

## Stage 1B: emitting IR from Ubel source

`crates/llvm_poc` is also a Rust crate, `ubel_llvm`. It runs the real front end (lexer,
recursive-descent parser, semantic analysis), so a program sema rejects never reaches the
emitter, then lowers a deliberately small subset to textual LLVM IR. Textual IR keeps
libLLVM out of the Cargo build; clang assembles it.

```
cargo build --release --manifest-path crates/llvm_poc/Cargo.toml
ubel-llvm emit   prog.ubl -o prog.ll   # IR
ubel-llvm interp prog.ubl              # the interpreter, for comparison
ubel-llvm check  prog.ubl              # front end only
```

### What the subset covers

Top-level functions with `int`, `bool` and `void` parameters and results; `@tier`
annotations (including `@tier(low) { ... }` blocks); `let`, `:=`, assignment and compound
assignment, shadowing in nested scopes; arithmetic, comparison, bitwise and short-circuit
logical operators; `if` / `elif` / `else` as statements and as expressions; `while`,
`loop`, `for i in a .. b` and `a ..= b`, `break`, `continue`, `return`, implicit return;
calls between functions; `println` and `print` of ints, bools, string literals and
interpolated strings; and `with arena(size) { ... }`.

Everything else is **refused with a located "not supported yet" error** instead of being
guessed at: structs, enums, strings as values, collections, closures, generics, `match`,
sized integers and floats. The test suite checks that each refusal names the right
feature, so unsupported code cannot be silently miscompiled.

### Tier lowering so far

Every tier shares one calling convention and data layout, so a cross-tier call is a plain
call; the IR marks it with a comment (`; HIGH -> MID call`) and records each function's
tier. The one tier-specific lowering is the arena block: `with arena(size)` becomes
`ubel_arena_create(size)` and the arena is destroyed on **every way out of the block**:
falling off the end, `return`, `break` and `continue`. The emitter keeps a stack of open
arenas and a depth per loop, so `break` and `continue` destroy only the arenas opened
inside the loop body. The language-level arena size is passed as the runtime arena's
chunk size, and the PoC arena grows; whether the size should be a hard limit is a language
decision still to make. Locals are `alloca` slots and there are no phi nodes; `mem2reg`
turns them into SSA at -O1 and above.

```
define i64 @ubel_early(i64 %p.0) {          ; from examples/09_arena_exits.ubl
  %arena.2 = call ptr @ubel_arena_create(i64 1024)
  ...
then2:
  call void @ubel_arena_destroy(ptr %arena.2)   ; destroyed before this early return
  ret i64 %t6
  ...
if.end4:
  call void @ubel_arena_destroy(ptr %arena.2)   ; and on the normal path
  ret i64 %t10
}
```

### How it is checked

`scripts/run_equiv.sh` compiles every program in `examples/` and compares the native build
with the interpreter. Native builds are run at -O0, -O2 and under ASan + UBSan when the
toolchain has them, with `UBEL_CHECK_LEAKS=1`, which fails the run if any arena is still
live at exit. Programs named `u_*` must be refused for the feature they use, and
`fail_*` programs must fail in both the interpreter and the native build with the same
output before the failure.

**CI results** (first run of the emit workflow, commit 22b0865): all 15 pass on both the
x86_64 and the aarch64 runner, with clang 18.1.3 and cargo 1.99.0, and the whole job took
50 seconds. That also confirms the pinned lockfile builds on a current cargo.

Local results (x86_64 sandbox, clang 18, Rust 1.75): all 15 pass, for
64-bit and `-m32` builds. Nine programs are identical to the interpreter on every platform tried (arithmetic,
loops, recursion, short-circuit order, scopes, `if` expressions, interpolation, calls
across all three tiers, and arena cleanup on every exit path), one fails identically
(division by zero), and five unsupported programs are refused for the right reason.

To check that the leak check can fail, the emitter was deliberately broken twice: with
cleanup skipped on `return` the suite went red (1070 arenas left alive), and with
cleanup skipped on `break` and `continue` it went red again (753).

### Known differences from the interpreter

`int` is 64-bit and arithmetic wraps natively on overflow; the interpreter's overflow
behaviour is not matched and the test programs avoid overflow. Division by zero traps in
both (exit status 1) but the messages differ. Shifts by 64 or more are undefined natively.
`INT_MIN / -1` is not handled.

### Findings about the front end

Found while writing the test programs; none is fixed here.

- `0..3` does not parse as a range (`expected identifier after '.'`); `0 .. 3` and
  `0 ..= 3` do.
- Redeclaring a name in the same scope (`let x = x + 10`) is an error; shadowing works only
  in a nested scope.
- In nested `for` loops over range literals, comparing the two loop variables
  (`if q > r`) fails sema with ``type `?T40` doesn't support ordering comparisons``. It
  looks like an inference gap in loop-variable types.
- `Program.items` includes the injected prelude (Option, Result and friends) with
  synthetic spans (`Span::is_prelude()`). Anything that walks the items must skip them.
- `SemaContext.expr_types` can hold unresolved type variables, for example for an `if`
  expression, and the unifier's substitution table is not exposed. A backend therefore
  cannot rely on `expr_types` alone for concrete types. The emitter derives types from its
  own lowering and uses `int_literal_types` to catch sized integer literals. A real backend
  probably needs sema to publish resolved types; this is a point to plan around.

### Toolchain notes

The root `Cargo.lock` is format v4, which needs cargo 1.78 or newer, and `miette`'s
`fancy` feature pulls in a `backtrace` release that needs Rust 1.81. To build the core
crate on Rust 1.75 the lockfile had to be marked v3 and `backtrace`, `addr2line`, `gimli`
and `object` pinned to older releases. The lockfile shipped with `crates/llvm_poc` is
already pinned that way and builds on both old and new cargo; the root lockfile is
untouched.

## Could the playground do this?

The question: the web playground runs Ubel in the browser by compiling the interpreter to
WebAssembly. Can the LLVM pipeline be offered the same way? Split into four parts, with what
was tested and what was not.

| part | verdict | evidence |
|---|---|---|
| **Show the generated IR** next to the program | **very likely feasible, small** | The emitter is pure Rust over the same core and parser crates the playground already compiles to wasm, with no libLLVM and no file access in the library. **Not tested**: this sandbox has no wasm32 Rust target. A `cargo check --target wasm32-unknown-unknown` step in CI would settle it. |
| **Run** the emitted code in a WebAssembly engine | **works** | Tested: the IR for all 10 runnable examples compiles with `clang --target=wasm32`, links with `wasm-ld` against a freestanding runtime (`wasm/wasm_rt.c`, printing and traps imported from JavaScript) and runs in Node with output identical to the interpreter and no arena left alive. The binaries are 0.5 to 1.4 KB. See `scripts/run_wasm_demo.sh`. |
| **Compile** IR to wasm inside the browser | **possible but heavy** | Browser builds of LLVM exist and accept LLVM IR and a WebAssembly target. One packaged browser compiler is about 60 MB and reserves 256 MiB of wasm memory; Wasmer's clang-in-browser demo is a 100 MB download. Not tried here. Only sensible as an opt-in feature, not the default playground load. |
| **HIGH tier** (the GC) on wasm | **needs design work** | The PoC collector finds roots by scanning the machine stack and registers conservatively. In WebAssembly the operand stack and locals are not addressable memory, so that cannot see them; only the linear-memory shadow stack is visible. HIGH on wasm needs precise roots, for example an explicit shadow stack that the compiler maintains. This is reasoning from how wasm works, not something tested here. `mmap`, `setjmp` and `__builtin_unwind_init` also do not exist there, so the runtime needs a port. MID and LOW are not affected the same way. |

What the demo did **not** cover: the Stage 1B subset never allocates from the GC heap and
never allocates from an arena (it only creates and destroys them), so the demo says nothing
about GC or arena allocation in wasm. Timings in a browser would not represent native
performance either, so this is a demonstration vehicle, not a benchmark.

A reasonable order, if the playground is pursued: first an "IR" panel (emit in the browser,
no compiler needed), which also shows the tier-boundary comments and the arena create and
destroy calls; then, optionally, an opt-in "run as compiled code" that loads an in-browser
LLVM only on demand; and only if browser-only execution of HIGH matters, work out precise
roots first. A second option is a small direct WebAssembly backend for the same subset, which
avoids the 60 MB compiler, at the cost of maintaining a second backend that does not exercise
LLVM. That trade-off is for later discussion.

## Validation still to do

None of this has been started, and the plan is open to change.

- **Compare against mature implementations.** The Stage 0 workloads should be written
  idiomatically in C, Rust, Go and C# (and perhaps Java), so Ubel's numbers sit next to
  real runtimes and not only next to each other. This needs a fair method: matching
  object sizes and allocation behaviour, warm-up, pinned CPUs where possible, and the
  same checksums.
- **Build a small real application in Ubel** during the PoC, as an end-to-end test of the
  compiler and the memory model. The candidate shape is an expression parser and evaluator
  like `ast_builder.ubl`, or a Mid Engine style workload (per-frame temporary data, ECS
  queries, packet processing). To be chosen together.
- **Benchmark methodology.** The report now prints the host CPU model (added after three
  runs showed the x86 pool varies). Still to add: warm-up runs, dispersion (for example
  median absolute deviation), the raw timing samples, explicit inputs and allocation counts,
  and hardware counters where available. The minimum of N runs is easy to
  flatter on shared runners.
- **Memory safety beyond checksums.** Equal checksums do not prove memory safety. The
  forced-GC self-test and the ASan/UBSan runs help; sanitizers cannot see through the
  conservative collector, so collector correctness needs its own stress tests.
- **GC research.** Notes on Go, Java, .NET and Swift collectors, with the claims that
  were checked and the ones that were wrong, are in `docs/GC_RESEARCH.md`.

## Limits of the runtime

- Objects up to 64 pointer-words: 512 bytes on 64-bit, 256 on 32-bit. Larger requests
  abort.
- HIGH zeroes memory; MID and LOW do not, as an allocator for each tier would.
- 32- and 64-bit targets are supported: sizes and pointer masks are derived from the
  target's data layout, and `run_bench.sh` sets the width of `size_t`.
- A scannable arena is scanned conservatively and in full at every collection (the whole
  of each retired chunk, the bumped part of the current one), so its cost grows with arena
  size.
- A plain arena that holds the only reference to a GC object is a bug, not a supported
  mode. `xit_arena_plain` exists only to demonstrate it.
- Globals are not GC roots, and nothing here is thread-safe.

## Bugs found while building this

- The first version assumed 8-byte words, so on a 32-bit target (Termux on `armv7a`) the
  collector read the wrong slot, never followed list `next` pointers and freed live
  nodes. Fixed by working in pointer-sized words and deriving layout from the target.
  `--selftest` exists so this class of problem shows up as a failure instead of a
  plausible-looking table.
- The tree workload's depth and repetition arguments were swapped in the harness wrapper.
  Checksums still matched, because all three tiers got the same wrong inputs, which is why
  the allocation counts are also worth checking.
- A delivery bug: a nested `README.md` in a replacements archive was captured by the
  standing manifest's filename-based override for `README.md` and overwrote the repo's
  root README. The same hazard applies to any nested `Cargo.toml`; the manifest needs an
  explicit entry for `crates/llvm_poc/Cargo.toml`.

## Next stages

To be planned together once Stage 1B's first slice has been reviewed. Open candidates:
grow the emitter toward `ast_builder.ubl` (structs, strings, closures, enums, `match`),
publish resolved types from sema, tier-aware allocation lowering for MID and LOW, the
cross-language baselines, the small Ubel application, and the GC research agenda.

## Log

- 2026-10-22: Stage 1B verified on CI (15 of 15 on x86_64 and aarch64, cargo 1.99.0).
  Benchmark run #4 recorded; three x86 runs show the runner pool is not uniform, so the
  report now prints the host CPU model. Browser-playground feasibility checked: emitted IR
  runs in a wasm engine (10 of 10 identical to the interpreter), in-browser LLVM exists but
  is large, HIGH-tier GC on wasm needs precise roots. `run_wasm_demo.sh` added.
- 2026-10-21 (later): Stage 1A run #3 recorded. Stage 1B first slice added: `ubel_llvm`
  emitter crate, 15 example programs, `run_equiv.sh`; matches the interpreter locally at
  -O0, -O2 and under ASan + UBSan, 64-bit and 32-bit. Arena cleanup on every exit path
  verified by deliberately breaking it. Provisional framing and validation plan added.
- 2026-10-21: Stage 1A added (`bench/boundaries.ll`): arena lifecycle sweep, callback and
  copy-out across MID/HIGH, iterator pattern with the hidden-root hazard and scannable
  arenas, calls into LOW. Self-tested.
- 2026-10-20: Stage 0 delivered; self-test and benchmark run on x86_64 and aarch64
  runners (run #1, above).
