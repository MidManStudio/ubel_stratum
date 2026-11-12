# LLVM proof of concept

Status: **Stage 0 and Stage 1A measured on CI (five runs). Stage 1B (emit IR from Ubel source): slices 1 and 2 (scalars, plain structs with per-tier allocation) verified on CI on x86_64 and aarch64; slice 3 (edge structs and references) is on master and slice 4 (enums and `match`) is in this change, both verified locally only so far. The plan from here is in `docs/LLVM_ROADMAP.md`. A browser-playground feasibility check is recorded below.**

> **Everything in this document is provisional.** This is a proof of concept. The
> numbers come from a hand-written runtime on two shared CI runners, the designs
> (closure ABI, scannable arenas, copy-out, arena lifecycle) are proposals, and none
> of it has been compared against mature implementations yet. Memory-management
> and cross-tier questions like these need proper research and benchmarking before
> any of them becomes a language or compiler contract. See "Validation still to do".

Everything lives in `crates/ubel_llvm/`. The C runtime and IR benchmarks depend on
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
| 1B | emit IR from Ubel source, compare with the interpreter | slices 1 and 2 verified on CI (15, then 21, then 22 of 22 on both runners); slice 3, edge structs, 27 of 27 locally, not yet run on CI; slice 4, enums and `match`, 36 of 36 locally, not yet run on CI |

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
crates/ubel_llvm/runtime/ubel_rt.h        API of the three allocation strategies and the print helpers
crates/ubel_llvm/runtime/ubel_rt.c        GC (size-class spans, tri-colour mark-sweep) and arenas
crates/ubel_llvm/runtime/ubel_main.c      main() for programs emitted by Stage 1B
crates/ubel_llvm/bench/tiers.ll           Stage 0 workloads, hand-written LLVM IR
crates/ubel_llvm/bench/boundaries.ll      Stage 1A: tier-to-tier calls and data, hand-written LLVM IR
crates/ubel_llvm/harness/bench_main.c     timing, checksum checks, markdown report, --require-gc
crates/ubel_llvm/scripts/run_bench.sh     build and run the benchmarks (also --selftest)
crates/ubel_llvm/Cargo.toml, src/         Stage 1B: the emitter crate (ubel_llvm)
crates/ubel_llvm/examples/*.ubl           Stage 1B test programs
crates/ubel_llvm/scripts/run_equiv.sh     Stage 1B: native build against the interpreter
crates/ubel_llvm/scripts/run_wasm_demo.sh Feasibility demo: emitted IR compiled to wasm32 and run in Node
crates/ubel_llvm/wasm/wasm_rt.c, run.js   freestanding wasm runtime and the Node host for that demo
```

## Running it

```
bash crates/ubel_llvm/scripts/run_bench.sh --selftest   # correctness first
bash crates/ubel_llvm/scripts/run_bench.sh              # Stage 0 and 1A benchmarks
bash crates/ubel_llvm/scripts/run_bench.sh --iters 9 --only tree
bash crates/ubel_llvm/scripts/run_bench.sh --only xing  # Stage 1A only (xing-lifecycle, xing-callback, xing-iter, xing-low)
bash crates/ubel_llvm/scripts/run_equiv.sh              # Stage 1B: emitter against the interpreter
bash crates/ubel_llvm/scripts/run_wasm_demo.sh          # emitted IR as WebAssembly, run in Node (needs wasm32 clang, wasm-ld, node)
```

The benchmarks need clang 15 or newer (opaque pointers). `EXTRA_FLAGS=-m32` and
`CLANG=clang-18` are supported. The Stage 1B script also needs cargo, because the
emitter is a Rust crate. On Termux: `pkg install clang`.

**Stress mode.** `UBEL_GC_STRESS=N` makes the runtime collect before every N-th allocation (`1` = before every
one; `ubel_gc_set_stress(N)` does the same from C). It is implemented by holding the trigger at zero, so a
normal run pays nothing for it. Anything that only a register or a half-built object holds, a missing
pointer-mask bit or a mis-sized object is then damaged at the very next allocation instead of whenever the
heap fills. `GC_STRESS=1 bash crates/ubel_llvm/scripts/run_equiv.sh` runs every compiled example that way
(the per-run timeout becomes `STRESS_TIMEOUT`, default 300 s; the runtime tests do not get it, they
count collections themselves). Slow: the allocation-heavy examples take 10 to 80 times longer.

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


### Run #5: same runners, after the struct slices

Commit 0d5079d, CI run number 5 of the benchmark workflow, the first with the host CPU
printed (x86_64: AMD EPYC 7763; aarch64: Neoverse-N2). The runtime's only change since run #4
is an attribute that exempts the stack scan from ASan instrumentation, which cannot change
code built without a sanitizer, and the benchmark programs are the same hand-written IR.

| runner | benchmark | HIGH (GC) | MID (arena) | LOW (malloc/free) | MID / HIGH | LOW / HIGH |
|---|---|---:|---:|---:|---:|---:|
| x86_64 | churn | 50.35 | 10.18 | 80.55 | 0.20 | 1.60 |
| x86_64 | churn+live | 116.75 | 10.89 | 80.97 | 0.09 | 0.69 |
| x86_64 | tree | 47.89 | 17.18 | 82.81 | 0.36 | 1.73 |
| aarch64 | churn | 44.75 | 15.62 | 63.87 | 0.35 | 1.43 |
| aarch64 | churn+live | 98.63 | 16.53 | 67.36 | 0.17 | 0.68 |
| aarch64 | tree | 41.83 | 14.65 | 71.81 | 0.35 | 1.72 |

- aarch64 reproduces run #4 in every ratio (0.35, 0.17, 0.35 and 1.43, 0.69, 1.72 before; 0.35,
  0.17, 0.35 and 1.43, 0.68, 1.72 now).
- x86_64 MID / HIGH matches run #4 (0.20, 0.09, 0.36 now; 0.21, 0.09, 0.37 before). **LOW did
  not:** LOW / HIGH on `churn` is 1.60 now against 1.17, and on `tree` 1.73 against 1.50.
  Compared with run #4, LOW's absolute time is 66% higher on `churn` (80.55 against 48.38 ms)
  while HIGH's is 22% higher and MID's 17%. That is larger than the earlier LOW/HIGH spread on
  x86 (0.99 to 1.21). It is not explained; it fits the already-noted x86 pool variance, and one
  more x86 run with the CPU printed would show whether it follows the host.
- GC activity on `churn+live`: x86_64 63.06 ms (54.0% of the run), 3.085 ms maximum pause;
  aarch64 51.36 ms (52.1%), 2.500 ms. Inside the earlier ranges.
- Boundaries: crossing cost 11 ns per depth-4 tree on x86 and 10 ns on aarch64 (the depth-9
  figures, 83 ns and -74 ns, are inside the known noise of about +/-50 ns); copy-out 13.0 to
  13.5 ns per node on x86 (earlier range was 6 to 11, same caveat as above) and 10.5 to 11.1 on
  aarch64; MID first beats HIGH and LOW at 2 objects per arena on both; the hazard demo still
  loses live results with a plain arena (253 collections) and passes with a scannable one.

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
  6.4 MB live set that was 52% to 59% of the run in all five runs. Collections are paced by the live size
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
- Roots are conservative; a global is a root only if its memory is registered (`ubel_gc_add_root`). The GC is a first cut and has not
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

Update (A7): the non-capturing case is implemented as proposed, except that the object is a
static constant rather than a GC object, because there are no captured values to hold. The
capturing case is implemented too (A7b): the object is allocated where the closure is written,
in the arena inside `with arena` and in the GC heap in a HIGH function, and holds the code
pointer and a copy of each captured int or bool, 8 bytes apiece.

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

`crates/ubel_llvm` is also a Rust crate, `ubel_llvm`. It runs the real front end (lexer,
recursive-descent parser, semantic analysis), so a program sema rejects never reaches the
emitter, then lowers a deliberately small subset to textual LLVM IR. Textual IR keeps
libLLVM out of the Cargo build; clang assembles it.

```
cargo build --release --manifest-path crates/ubel_llvm/Cargo.toml
ubel-llvm emit   prog.ubl -o prog.ll   # IR
ubel-llvm interp prog.ubl              # the interpreter, for comparison
ubel-llvm check  prog.ubl              # front end only
ubel-llvm build  prog.ubl -o prog      # a native executable (needs clang 15 or newer)
```

### Building an executable

`ubel-llvm build prog.ubl -o prog` is the whole route from Ubel source to a native executable (M1a of
`LLVM_BACKEND_STATUS.md`; provisional). It is `ubel_llvm::driver::build` underneath, so the `ubel` command line can
call the same code later (M1b).

1. `compile` runs the front end and the emitter. A program that is wrong, or that uses something the emitter does not
   support yet, stops here with the located messages, before any compiler is looked for.
2. The IR and the C runtime (`runtime/*.c` and `runtime/*.h`, embedded in the crate with `include_str!`) are written to
   a private temporary directory (mode 0700, `ubel-build-<pid>-...` under `$TMPDIR` or `/tmp`).
3. One clang invocation assembles the IR, compiles the runtime and links: `clang -O2 [flags] -Wno-override-module
   -I DIR DIR/program.ll DIR/ubel_rt.c DIR/ubel_main.c -o prog`, the line `run_equiv.sh` uses. The runtime is compiled
   again on every build; there is no cache. One measurement, example 19 on the sandbox machine: about 0.5 s at `-O2`
   and 0.2 s at `-O0`.
4. The temporary directory is removed again, also when the build fails.

Options: `-o PATH` (default: the file name without its extension, in the current directory; refused when that is the
source file itself), `--cc PATH`, `-O0 -O1 -O2 -O3 -Os` (default `-O2`), `--keep-temps` (keep the directory and print
where it is), `-v` (print the compiler and the full command), and `-- FLAGS` (everything after goes to clang, for
example `-m32` or a sanitizer flag). Exit status 0, 1 on any error, 2 on a usage error. On success it prints one line,
`built <path>`, to stderr.

The compiler is found like this: `--cc`, else `$UBEL_CC`, else the first of `clang-18`, `clang-17`, `clang-16`,
`clang-15` and `clang` on the PATH that runs and says `clang version N` with N at least 15. Apple's clang numbers its
versions differently and is not held to the minimum. A compiler that was named is never replaced by another one;
during the automatic search an unusable candidate is skipped, and the first problem is reported if nothing works.

The Unicode tables the runtime carries are the committed `runtime/ubel_unicode.h` (Unicode 15.0.0 today). The driver
embeds that file as it is and does not regenerate it; `run_equiv.sh` still does when its toolchain differs.

Tested (`crates/ubel_llvm/tests/build.rs`, and the unit tests in `src/driver.rs`): eight programs, among them the
target program, built through the driver and equal to the interpreter; a program that fails at run time (a build that
works, a native exit status 1); the front end and emitter errors arriving before the compiler is looked for; a missing
compiler, a missing output directory, a clang failure with its message and command; flags reaching clang; the
temporary directory removed, kept on request and private; the whole command line including the refusal to overwrite
the source; and the compiler search order, checked with fake compilers. Since M2a the examples were also built and compared with clang 15.0.7, 19.1.1 and 20.1.2 (all
passed, as with 18.1.3), and on emulated 32-bit ARM and aarch64 Linux (see `ANDROID_TERMUX.md`). **Not tested:**
Windows (the `.exe` suffix and the `.exe` search are written and never run), macOS, Termux itself and Android's libc,
a clang newer than 20, and any CI run. Android: `ANDROID_TERMUX.md`.

### What the subset covers

Top-level functions with `int`, `bool` and `void` parameters and results; `@tier`
annotations (including `@tier(low) { ... }` blocks); `let`, `:=`, assignment and compound
assignment, shadowing in nested scopes; arithmetic, comparison, bitwise and short-circuit
logical operators; `if` / `elif` / `else` as statements and as expressions; `while`,
`loop`, `for i in a .. b` and `a ..= b`, `break`, `continue`, `return`, implicit return;
calls between functions; `println` and `print` of ints, bools, string literals and
interpolated strings; and `with arena(size) { ... }`.

**Integers and `assert` (M2a, provisional).** Besides `int` (64-bit and signed; `long`, `i64` and `isize` are the same
type for the emitter), the fixed-width integers `i8 i16 i32` and `u8 u16 u32 u64` work wherever `int` does: locals,
parameters, results, struct fields, enum payloads, closure parameters, captures and results. The other spellings map as
sema maps them: `byte` is `i8`, `ubyte` is `u8`, `uint` is `u32`, `ulong` and `usize` are `u64`. Sema gives each
literal its width (`int_literal_types`, or a suffix such as `250u8`). Every one is a plain LLVM integer; the signedness
is in the operation chosen (`sdiv` or `udiv`, `ashr` or `lshr`, `slt` or `ult`, `sext` or `zext`), and arithmetic wraps
(no overflow flags). The rules follow the interpreter's sized path (`sized_int_binop`), checked by examples 33 to 35
over every operator at every width:

- a shift amount is masked to the width (`b & (bits - 1)`), so a shift by the width or by a negative amount is
  defined (LLVM calls it poison, hence the explicit `and`; this also fixes `int`, which was emitted unmasked);
- `MIN / -1` is `MIN` and `MIN % -1` is 0 (LLVM's `sdiv` and `srem` are undefined there and trap on x86, so a divisor
  of -1 is replaced by 1 and the quotient corrected);
- a division or remainder by zero stops the program at every width (exit status 1, message on stderr);
- `-x` wraps for the signed widths; negating an unsigned integer is refused (the interpreter panics at run time);
- `x as T` between integer types, and from `bool`, follows Rust's `as`: a narrower target truncates, a wider one extends
  by the *source's* signedness, the same width reinterprets. `short` and `ushort` are refused: sema and the interpreter
  both widen them (`short` is `int`, `ushort` is `u32`; `70000 as short` is `70000`), which looks like a placeholder, so
  the emitter does not copy it. Write `i16` or `u16`.

Printing and interpolation widen to 64 bits (sign- or zero-extended); `u64` has its own unsigned helpers
(`ubel_print_u64`, `ubel_u64_len`, `ubel_u64_write`). Not yet: `float` and `double` (M2b), the sized integers as a `for`
loop variable or in a `match`, and `as` to or from `char` or `string`.

`assert(cond)`, `assert(cond, "message")`, `panic()` and `panic("message")` call `ubel_panic`: flush standard output,
print the message to standard error, exit with status 1. Every argument is evaluated first and in order, as the
interpreter does (checked with a message that prints), and a message must be a string; other values are refused, because
the interpreter prints any value. The suite compares standard output and the exit status, not standard error, so the
wording of a panic message is not checked.

**Structs (slice 2).** Plain structs whose fields are `int`, `bool` or another struct:
literals, field reads, field assignment (plain and compound, nested too), and passing and
returning them. A struct value is a pointer, matching the interpreter, where structs have
reference semantics: `let b = a` makes an alias, and a callee that writes a field changes
the caller's struct (probed, not assumed; see the front-end findings below). At most 32
fields per struct, so every object fits the runtime's size limit on 32-bit targets.

**Edge structs and references (slice 3).** `edge struct Name [lifetime L]` is accepted, and a
reference to a struct is the same pointer as the struct value: `&x`, a `&L T` field, a
parameter or result of type `&L T`, and a `[lifetime L]` on a function. The lifetime is
sema's business and is erased here, so edge structs need no layout of their own. A `&L T`
field counts as a pointer field for the collector's mask. Field chains through references
(`call.arg.lhs.value`) work like any other field chain. Refused, by name: `&mut`, an
explicit `*` dereference, a reference to anything but a struct (`&L int`), optional
references (`&L T?`).

**Enums (slice 4).** Enums with payload-free and tuple-payload variants, constructors
(`Expr.Add(a, b)`, `Color.Red`), and `match` as a statement or an expression. **Provisional
layout:** every enum value is a pointer to an object: an 8-byte tag (the
variant's position in the declaration), then as many 8-byte payload slots as the widest variant has,
so all values of one enum have one size. A pointer payload sits at the start of its slot. The
collector mask depends on the variant (only some slots hold pointers), so it is computed per
variant from the target's layout, like a struct's. Allocation sites follow the struct rule: inside
`with arena` bump allocation, in HIGH the GC heap, elsewhere refused (for a variant with a payload). **A
variant with no payload (E1, provisional) is not allocated at all:** it is a static constant object (its tag
and zeroed slots, `@.enumv.Enum.Variant`) outside the heap, like a string literal, so it is legal in every
tier, including LOW and MID outside an arena, where building an enum value used to be refused, and the
collector ignores pointers to it. Nothing writes into an enum object after it is built, so one object per
variant is shared. Measured once, on a 3,000,000-step loop over a three-variant enum at -O2: 3,000,001
allocations, 48 MB and 11 collections (about 25 ms) before, none of them (about 10 ms) after; that loop is all
construction, so it is a best case. A variant that carries
a discriminant is treated as payload-free (sema validates the value and nothing reads it back).
`match` lowers to a `switch` on the tag; a tag no arm covers traps, so a hole in sema's
exhaustiveness check cannot become undefined behaviour. Patterns supported: `Enum.Variant(a, _)`,
bare payload-free variant names, `_`, and a plain name that binds the whole value. Refused by
name: struct-style variants and patterns, `where` guards, nested patterns, `mut` bindings,
generic enums, printing an enum value and `==` on enums
(the interpreter prints `C.Red` and compares by variant; the emitter does neither yet). At most 31
payload fields per variant. An arm such as `=> return n,` is parsed as a one-statement block (the
parser treats `return`, `break`, `continue` and `fail` as statements), so arm bodies are lowered as
blocks or expressions, and a `match` used as a value skips the arms that leave the function or loop.

**Strings (A6, provisional).** `string` works as a parameter, result, local, struct field and enum
payload type. A string value is a pointer to `{ i64 length, bytes }` (UTF-8, no terminator,
immutable). A literal is a static constant outside the heap, so the collector ignores pointers to it
and nothing ever frees it. `$"..."` builds a string (decision D3b, `docs/ARENA_STRINGS.md`): every
hole is evaluated first, in order; then one object of `8 + length` bytes is allocated where the
literal is written, and the pieces are copied in. Holes may be ints (decimal), bools (`true` or
`false`) or strings; format specifiers are refused. In a HIGH function the object is a GC object with
mask 0, so the collector never scans it. Inside `with arena` it is bump-allocated, but only when sema
tracks arena strings (the D3b sema patch); without that the emitter refuses, because nothing would stop
the string leaving the block. Refused as for struct literals: LOW, and MID outside an arena.
`==` and `!=` compare length and then bytes; `<`, `<=`, `>` and `>=` compare bytes in UTF-8 order, like
the interpreter's string comparison (`ubel_str_cmp`; a shorter string that is a prefix of a longer
one comes first, and `"é"`, the bytes C3 A9, comes after every ASCII letter). `a + b` copies both
operands into a new string that is **always allocated in the GC heap**, wherever it is written (rule
S4 of `docs/ARENA_STRINGS.md`: the copy holds no borrow of its operands, so it may leave a `with arena`
block even though an arena string cannot); it is refused in LOW and in MID outside `with arena`, where
GC allocation is not allowed or not decided. `+=` works through the same path. Any other operator on
strings is refused. `println` prints a string value. A string field of a GC
struct or enum is a pointer in the collector mask; a string field of an arena object is safe because
such an arena is created scannable (see "Scannable arenas"). Runtime helpers: `ubel_str_eq`, `ubel_i64_len`, `ubel_i64_write`, `ubel_put`.
`examples/19_strings` mirrors the target's `show` and `apply` and then builds about 20 MB of garbage
strings, so strings held only by a struct field and an enum payload must be kept by the collector. To
check that it can fail, `Ty::Str` was taken out of the pointer mask: the output came back as
`label-71(label-72)` instead of `fn7(v1)` (freed and reused memory). Mutants of `ubel_str_eq` and of
the minus sign in `ubel_i64_len` are caught too.

**String methods (A9, provisional).** The methods that read a string and allocate nothing work: `len` (the
byte length stored in the object, as in the interpreter: `"é".len()` is 2 and `"日本".len()` is 6),
`is_empty`, `contains`, `starts_with` and `ends_with`. They hold no borrow of the string, so they are legal
in every tier and on arena strings, as receiver and as argument; sema needs no change (it checks only the
argument count of a built-in string method, so an arena argument passes). `len` and `is_empty` are inline
loads of the stored length; the other three call `ubel_str_contains`, `ubel_str_starts_with` and
`ubel_str_ends_with`, which work on bytes (a valid UTF-8 needle can only match at a character boundary, so
the answers are the interpreter's). An empty needle is found everywhere and a needle longer than the string
nowhere, as in Rust; `contains` is a plain scan, O(n*m) in the worst case where Rust's search is linear. The
receiver is evaluated before the argument, like the interpreter. Refused: `contains`, `starts_with` and
`ends_with` with an argument that is not a string (sema accepts it and the interpreter answers `false`; the
emitter does not copy that); and the ones that return a list (`chars`, `split`: refused as a value of type
`List<string>`). The methods that build a new string work too (A9c, below). The interpreter has no `find` and no `substring`. Not probed: methods that come from the
prelude traits (such as `lt`); they fall under the catch-all refusal by construction, not by a test.
Checked by `examples/27_string_methods` (byte lengths, empty needles, restarting partial matches, receivers
that are literals, calls, fields, captures and arena strings, a 100,000-byte string, 400,000 allocations),
`rt_gc_test str_helpers` (a table of answers Rust gives, and a string with the rest of the needle right
behind it in memory, so a helper that trusts the needle's length reads past the string and matches) and the
refusal examples `u_string_list` and `u_string_method_arg`.

**String match (A9b, provisional).** `match` on a string works. The arms are string literals (plain or
verbatim), `a | b` of string literals, `_`, and a plain name that binds the string (a catch-all). The tests run
in arm order as a chain of `ubel_str_eq` calls, so the first matching arm wins, as in the interpreter (a
repeated literal, and any arm after a catch-all, never runs); the scrutinee is evaluated once. Matching
allocates nothing, so it works in every tier. The cost is the number of literals times a length compare and,
when the lengths agree, a `memcmp`; there is no hashing and no decision tree. Refused: guards (`where`, as for
enums), `mut` bindings, and any other pattern kind. **When no arm matches, the interpreter does not fail: it
gives a unit value and carries on** (`println(f("c"))` prints an empty line), and sema does not ask for a
catch-all on a string `match`. Native code cannot copy a unit for an `int`, so a `match` used as a value must end
in `_` or a name (refused otherwise, `u_match_string_value`); used as a statement without one, nothing happens
when no arm matches, as in the interpreter. Found: sema rejects a `match` whose scrutinee is an arena string (it
types the literal patterns `&arena string` and reports that they cannot outlive the block), so that case never
reaches the emitter; see `docs/ARENA_STRINGS.md`. Checked by `examples/28_string_match` (literals including the
empty string, non-ASCII and escapes, or-patterns, a repeated literal, a name that binds, strings in and out of
enums, statements with block arms and `return`, scrutinees that are calls, concatenations, interpolations,
fields and captured strings, every tier, long strings, 200,000 matches across many collections) and the
refusal examples `u_enum_string` (now an integer `match`), `u_match_string_guard` and `u_match_string_value`.

**String methods that build a string (A9c, provisional).** `to_upper`, `to_lower`, `trim`, `trim_start`,
`trim_end` and `replace` work. Each calls a runtime helper (`ubel_str_to_upper`, ..., `ubel_str_replace`) that
returns a **new string**, a copy even when nothing changes.
**Where it lives** (decided by the owner, 2026-11-08): in the receiver's own arena when the receiver is an arena
string, otherwise in the GC heap (before that decision it was always a GC copy, rule S4 as for string `+`).
Sema types the result of a method on an arena receiver with the receiver's arena tag (`a.trim().to_upper()` cannot
be returned out of the block; probed), and that tag carries the `ArenaId` of the receiver's arena, so the
emitter allocates in exactly that arena: it learns which arena pointer an `ArenaId` stands for where it allocates
an interpolated string in an arena (sema tags those with the innermost arena at that site), and forgets it when
the block ends. A receiver from an OUTER arena used inside an inner block therefore gets its result in the outer
arena, not the inner one (`examples/32_string_arena`, `nested`: a second inner arena reuses the first one's memory,
so a misplaced result would read as garbage or trip the sanitizers). When the receiver's arena is not one of this
function's open arenas (a borrowed `&arena string` parameter, a closure body) there is no arena pointer to
allocate in, and the result is a GC copy, which is stricter than its type needs, never looser. The helpers take
the arena as a last parameter (`NULL` means the GC heap), and the IR call shows the choice (`ptr %arena.N` or
`ptr null`). The methods are legal in a HIGH function and inside `with arena`, and refused in `@tier(low)` and in
`@tier(mid)` outside `with arena`, in the wording of the `+` refusals.
The semantics are Rust's, because the interpreter calls Rust's own `str::to_uppercase`, `to_lowercase`,
`trim`, `trim_start`, `trim_end` and `replace`: the full Unicode case mappings (`"straße".to_upper()` is
`"STRASSE"`, `"ǅ"` maps to `"Ǆ"` and `"ǆ"`, `"ŉ".to_upper()` is `"ʼN"`, `"İ".to_lower()` is `"i̇"`), the
final-sigma rule (`ΑΣ` lowercases to `ας` but `ΑΣΑ` to `ασα`; the context skips Case_Ignorable characters, so `Α.Σ`
gives `α.ς`), `trim` removes Unicode White_Space (no-break space, U+3000, NEL; not U+200B or U+FEFF), and
`replace` is non-overlapping, left to right, with an empty needle matching at every character boundary
(`"abc".replace("", "-")` is `"-a-b-c-"`). The tables are `runtime/ubel_unicode.h` (53 KB), **generated** by
`scripts/gen_unicode.rs` from the standard library of the toolchain that runs it: White_Space (10 ranges), the
upper and lower mappings that differ from the identity (1,525 and 1,433 entries, with a 222-code-point pool for
the expansions such as `ß` to `SS`), and the two sets the sigma rule consults. `Cased` and `Case_Ignorable` are
not public on stable Rust, so the generator measures them through `str::to_lowercase`; a character that is both
counts as ignorable, as in Rust's rule, so the table called `cased` holds the cased characters that are not
ignorable, which is all the rule can find after skipping. Lookups are binary searches with an ASCII fast path;
a stray byte that is not valid UTF-8 is passed through unchanged. **The tables are per toolchain**: the
interpreter uses whatever Unicode version its Rust has, and the committed header is Unicode 15.0.0 (Rust 1.75,
the only toolchain I had), while CI builds with the runner's stable Rust, which is newer. So `run_equiv.sh`
compares its toolchain's Unicode version with the header's and regenerates the header and
`examples/29_string_unicode.ubl` when they differ (tested by faking the version: the regenerated files were
identical to the committed ones; commit the regenerated files to keep them). Characters added to Unicode after
15.0 are therefore only as right as the last regeneration. Checked by `examples/29_string_unicode` (generated;
528 `show` and 200 `replace` calls: every code point whose upper or lower mapping is not itself, so every table
entry; the white-space set and look-alikes; the final-sigma rule around both edges of every Cased and
Case_Ignorable range; random strings from a pool of awkward characters; random `replace` calls), by
`examples/30_string_build` (readable cases, chaining, receivers of every kind, closures, an arena, 100 KB strings,
a 100,000-iteration loop across many collections), by `examples/32_string_arena` (all three kinds of arena
receiver; zero GC allocations in the whole program) and by the refusal examples `u_string_value` (repurposed:
LOW), `u_string_build_mid` and `u_string_replace_arg`. `rt_gc_test str_build_arena` (17 tests now) checks that a
helper given an arena leaves the collector's allocation count alone and bumps the arena by the rounded size, and
that one given NULL allocates exactly one GC object. An example may carry `// expect-ir: <text>` and
`// expect-no-ir: <text>` lines, which `run_equiv.sh` checks against the emitted IR; 32 uses them to require that
no building call passes a null arena.

**Scannable arenas (provisional).** A `with arena` block is created with `ubel_arena_create` unless
an object allocated in it has a pointer-typed field (a string, struct, enum or closure: a struct or
enum literal with such a field, or a closure that captures such a value), in which case it is created
with `ubel_arena_create_scan` and the collector treats what was written in it as roots. This is decided
after the block has been emitted, by rewriting the creation call, because it depends on what the block
allocates. The test is on the field's type, not on where the value came from, so it is conservative: an
arena object whose pointers all lead to other objects of the same arena also makes its arena scannable,
which costs a scan of the written bytes at each collection and a registry entry per arena, nothing
else. The runtime scans only the bytes that were written: below `cur` in the current chunk, and the
`used` count recorded in each chunk's header when the arena moved on in the retired ones, so stale
bytes in recycled memory cannot keep garbage alive (they used to: a retired chunk was scanned in
full). Plain arenas stay plain, and are still the fast path for blocks that hold only ints and
bools. `examples/25_arena_scan` has an arena as the only holder of GC strings (in a struct field, in
enum payloads and in a captured variable) while a closure builds hundreds of thousands of garbage
strings of the same size, so a collection runs inside the block and any string the collector did not
keep would come back overwritten; a chain of 64 GC closures, each the only holder of its captured
string, tests the mask. `rt_gc_test arena_scan` checks the scan extent: an object referenced only from
the unwritten tail of a retired chunk is collected, ones referenced from written bytes are kept. Mutants
that scan nothing, that skip the enum or the closure flag, that zero the GC closure mask, and that scan
a retired chunk in full (the old behaviour) are all caught. The struct-site flag is not caught by its
own mutant: the string pointer also sits in a spill slot or callee-saved register of the creating frame,
and the conservative stack scan keeps it alive; the struct and enum sites share `mark_scan` and the
enum site is caught.

**Closures that capture nothing (A7, provisional).** A function type (`fn(int) string`) works as a
parameter, result, local and struct field type, and `fn(v: int) body` is a value of it. A closure is a
pointer to a static object `{ ptr code }` outside the heap: creating one allocates nothing, and the
collector ignores pointers to it. Its body is emitted as its own function whose first, hidden
parameter is the closure object; calling a local of function type loads the code pointer from the
object's first word and calls it indirectly (the Stage 1A proposal, without captured values). The body
is emitted in the tier of the function it is written in, so a closure written in HIGH code allocates in
the GC heap whatever tier calls it, and one written in MID code outside `with arena` is refused if it
allocates. Parameter types must be written (`fn(x) ...` is refused). The result type is sema's when it
resolved; otherwise it is taken from the first value the body returns, because sema records a lambda's
type before an integer literal in the body has been defaulted to `int` (a block body that returns such
a local has an open result type there). Refused by name: calling a closure in a LOW function unless it
was created there (see D2 below); calling one that is not a plain local (read it out of a struct field first); printing or comparing closures. `examples/21_closures` covers two parameters, block bodies, nesting, closures returned and
kept in a struct field, a callback called from MID inside `with arena`, and 100,000 calls whose bodies
build strings while the collector runs. Mutants that reversed the arguments, did not restore the temp
counter or did not restore the enclosing scopes are all caught.

**The tier of a closure (D2, provisional).** A closure has the tier of the function it is created in, and its
body is checked and compiled in that tier (the tier checker walks a lambda under the enclosing function's
tier, so a lambda written in a LOW function cannot call a MID function). The tier matters where a closure
is *called*, and a function type carries no tier, so the tier of what a variable holds is known only when
the variable was bound where the closure was created. The rule, in sema (`TIER-017`,
`LowCallsUnknownClosure`, `tier_check.rs`) and independently in the emitter: **a LOW function may call a
closure only if it is an immutable local bound by `let` straight to a lambda in that function** (or captured
from one that is). A parameter, a `mut` binding, a closure returned by a call (even from a LOW function) and
anything else could belong to a higher tier, so calling it from LOW is refused. MID and HIGH may call any
closure: handing a closure down to a MID function that calls it is the callback pattern, which the target
program is built on, so the rule that a named call may not go up a tier is deliberately not applied to
closures. Only LOW can be checked because only LOW has a premise to protect (it runs nothing that can
allocate or trigger a collection). The conservative parts are real limits: a closure from a LOW function that
returns one cannot be called from LOW, and a tier in function types (so that a MID parameter could say what
it accepts) would be a language change. `examples/26_closure_low` creates and calls closures in LOW
functions; `tests/fixtures/*_closure_tier_*` pin the rule.

**Closures that capture (A7b, provisional; decisions D2 and D5, taken by the project owner on
2026-10-31).** D2: a closure's tier is the tier of the function it is created in; its body is
compiled in that tier, which is how the emitter already worked (see "The tier of a closure" below
for what that means at a call site). D5: a capturing closure's object
lives in the arena of the block it is written in and dies with it. Read as "by site", like a struct
literal: inside `with arena` it is bump allocated in the arena; in a HIGH function, where there is no
arena, it is a GC object (its mask marks the pointer-typed captures). **That second case is my
reading of D5, not something that was stated: confirm or reverse it.** MID outside an arena and LOW are
refused, as for a struct literal. A capture is a copy of the value taken when the closure is created;
the body copies it out of the object at entry (a prologue after the allocas). Captures are found while
the body is emitted: a name that is not a local of the closure but is one of the frame it is written
in becomes a capture. Only a variable that never changes can be captured, so `let mut` variables, `for`
variables and `:=` bindings are refused. Any other value is captured: ints and bools by value, strings,
structs, enums and closures as the pointer (so a captured struct is shared with the variable it came
from, as structs always are), and a captured closure can be called inside the closure that captured it.
A closure object is 8 bytes of code pointer and 8 bytes per capture; the collector's mask of a GC closure
object is worked out in the IR from the target's pointer size (a slot is two words on a 32-bit target), so
one `.ll` is right for both widths, and a closure may capture at most 30 variables. (The first version
hard-coded 64-bit word positions; `25_arena_scan` failed at 32-bit and found it.)
Refused by name as well: a capture from more than one level out (a closure in a closure that uses a
variable of the outer function), and any closure written inside a closure written inside `with arena`
(the inner body does not hold the arena, so there is nowhere to allocate it). Sema needed two fixes
(patch `type_infer.rs`, tests/fixtures `*_arena_closure_*`): a closure written inside `with arena` is
typed `&arena fn(...)` and could not be called at all ("no method `<call>`"), and the error then
panicked the type checker, because the error sentinel reached `apply` and `unify`. The tag still stops
the closure from being returned, assigned outward or passed to a function. `examples/22_closure_capture`
(GC-heap closures, no sema patch needed) and `examples/23_closure_arena` (needs the patch) cover
captured ints and bools, if-expression and block bodies, closures made in a loop, nesting, closures
returned and kept in a struct field, and 300,000 rounds of same-size GC closure objects. Mutants of the
capture offset, the object size, the `mut` check and the collector's pointer mask are all caught (the
last one only after the churn was made the same size as the object it must not free).

Everything else is **refused with a located "not supported yet" error** instead of being
guessed at: collections, generics, struct methods,
`edge struct`, `match`, sized integers and floats; also printing or comparing a struct value. The test suite checks that each refusal names the right
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

**Struct allocation follows the tier the literal is written in:**

| where the literal is | lowering |
|---|---|
| inside `with arena` (MID) | bump allocation from the innermost open arena, fast path inlined (`@ubel_arena_bump`), `ubel_arena_grow` on a full chunk |
| in a HIGH function | `ubel_gc_alloc(size, ptrmask)`; size and mask come from the target's data layout (`getelementptr null` constants), so one `.ll` is right for 32- and 64-bit |
| in a MID function outside `with arena` | **refused**: the language does not say where that allocates (MEMORY_MODEL.md, open decision 3) |
| in a LOW function | **refused**: how LOW allocates is undecided, and `Unique.new` cannot serve as a reference because the interpreter panics on a field read through it |

Field expressions are evaluated first, in source order, then the object is allocated and
filled. For a field assignment the object expression is evaluated before the right-hand
side; the interpreter's order for that case was not probed, and the tests do not depend on it.

An arena object that holds a pointer to a GC object is only safe if the collector can see that
pointer. Originally that held by accident: in this emitter a MID function could not
build GC objects (its literals outside an arena are refused), so the only GC pointers it can
store came in as parameters, and the caller's frame still holds them. That stopped being true once
closures could return GC objects into MID code (the Stage 1A hidden-root hazard); it is handled now by
scannable arenas, below.

```
define i64 @ubel_early(i64 %.arg0) {        ; from examples/09_arena_exits.ubl
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

Local results for slice 1 (x86_64 sandbox, clang 18, Rust 1.75): all 15 pass, for
64-bit and `-m32` builds. Nine programs are identical to the interpreter on every platform tried (arithmetic,
loops, recursion, short-circuit order, scopes, `if` expressions, interpolation, calls
across all three tiers, and arena cleanup on every exit path), one fails identically
(division by zero), and five unsupported programs are refused for the right reason.

To check that the leak check can fail, the emitter was deliberately broken twice: with
cleanup skipped on `return` the suite went red (1070 arenas left alive), and with
cleanup skipped on `break` and `continue` it went red again (753).

**Slice 2, structs (verified on CI: run #2 at d0d34d5, 21 of 21 on x86_64 and aarch64, ASan+UBSan included, which also confirms the stack-scan exemption works on CI).** 21 programs: twelve runnable (three new),
one failing, eight refusals. All 21 pass on x86_64 64-bit, and all 21 pass as 32-bit i386
binaries (`EXTRA_FLAGS=-m32`; the output is a 32-bit ELF), with the same `.ll` file.
`examples/10_struct_basic` covers aliasing, field writes through a callee, nested structs
and structs from `if` expressions. `examples/11_struct_gc` replaces a three-level chain
over and over while garbage forces collections, so each new object is reachable only through
a field of an older one, and checks a second chain that must survive. `examples/12_struct_arena`
covers arena allocation including chunk growth, early exits after allocating, nested arenas,
and a HIGH struct written through by MID code and read by LOW code. To check that
`11_struct_gc` can fail, the pointer mask was deliberately forced to zero: the program
crashed with a segmentation fault, with and without a lowered collection trigger. The
refusals each name the right feature (struct methods, MID literal outside an arena, LOW
literal, printing a struct).
The wasm demo skips the three allocating examples instead of failing them (10 passed,
3 skipped), because the wasm runtime has no GC and no arena memory.
The harness also guards the target program: the interpreter must run
`examples/arena_demo/ast_builder.ubl` and print `ast_builder.expected` (checked by hand),
and, since A8, the native build runs in the equivalence loop like every example (22 checks in all
when CI run #3 at 0d5079d passed on both runners; the count has grown since). Corrupting the
expected file turns the run red.

**Slice 3, edge structs and references (local only, not yet on CI).** 27 checks: fourteen
runnable programs (two new), one failing, eleven refusals (three new), and the target-program
guard. All 27 pass at 64-bit and as 32-bit binaries. `examples/13_edge_arena` is part 2 of the
target program (the same node shapes, `build_and_eval`, `checksum`) without the callback,
enum and strings, plus early exits after linking nodes in an arena. `examples/14_edge_gc`
builds a Pair and two Leaves per call in the GC heap, returns a `&L Pair`, and replaces a
Holder's field again and again under heavy garbage, so the newest objects are reachable only
through `&L` fields of older ones. To check that `14_edge_gc` can fail, the pointer mask was
deliberately forced to zero: the program crashed with a segmentation fault and printed
nothing (an earlier attempt at this check had a path error and proved nothing; it was redone).
The wasm demo now skips five programs (10 passed). The target program's first refusal moved
from `edge struct` to `enum declarations`.

**Slice 4, enums and `match` (local only, not yet on CI).** 36 checks: seventeen runnable
programs (three new), one failing, seventeen refusals (six new), and the target-program guard.
All 36 pass at 64-bit and as 32-bit binaries; the wasm demo skips eight programs (10 passed).
`examples/15_enum_basic` covers payload-free and payload variants, `match` as a statement and as
an expression (one arm a block), bare variant names, `_`, a name binding the whole value, an
enum in a struct field, a struct in a payload, and a recursive enum evaluated by recursion.
`examples/16_enum_gc` builds a fresh recursive tree every round and stores it in a field of an
older struct under heavy garbage, so the newest tree is reachable only through that field and
through the pointers in its own payloads. `examples/17_enum_arena` builds enums inside `with arena`
and leaves the arena from inside `match` arms with `return`, `break` and `continue`. To check
that `16_enum_gc` can fail, the per-variant pointer masks were forced to zero: the first line of
output still matched, but the surviving tree evaluated to 246 instead of 7 (freed and reused
memory, no crash), so the test catches silent corruption as well as crashes. The target
program compiles now (A7, A8: see "The target program").

### Known differences from the interpreter

**Plain `int` arithmetic in the interpreter (found in M2a; a core bug, not fixed here).** For two `int` values the
interpreter converts both to `f64`, computes, and converts back (`promote_numeric` in `eval_binop`), although
`Value::Int`'s own comment and `PRINT_FORMAT_RULES.md` describe 64-bit wrapping arithmetic. Past 2^53 it loses precision
and on overflow it saturates instead of wrapping. Measured, each line interpreter against native:

| Expression | Interpreter | Native (and the documents) |
|---|---|---|
| `9007199254740993 + 0` | `9007199254740992` | `9007199254740993` |
| `9223372036854775807 + 1` | `9223372036854775807` | `-9223372036854775808` |
| `9223372036854775807 * 2` | `9223372036854775807` | `-2` |
| `9223372036854775806 % 10` | `8` | `6` |
| `-9223372036854775808 / -1` | `9223372036854775807` | `-9223372036854775808` |
| `9007199254740993 > 9007199254740992` | `false` | `true` |

The last row is the worst: in the same program the interpreter says `a > b` is false and `a >= b` and `a != b` are true.
Native is right in every row. The two agree for values below 2^53 that do not overflow, which is all the `int` examples
use. The sized integers do not have the problem (their interpreter path, `sized_int_binop`, is exact and wrapping),
which is why examples 33 to 35 use them for the edge cases. A fix would add an `(Int, Int)` arm with `wrapping_*`
operations in front of `promote_numeric`; it is the owner's file, so this is a suggestion and not a patch.

Division by zero stops both with exit status 1, but the messages differ, and so do the messages of `assert` and `panic`
(the suite does not compare standard error). Shifts and `MIN / -1` are defined natively the way the interpreter defines
them for the sized integers (see "Integers and `assert`").

### Findings about the front end

Found while writing the test programs; none is fixed here.

- `0..3` does not parse as a range (`expected identifier after '.'`); `0 .. 3` and
  `0 ..= 3` do.
- Redeclaring a name in the same scope (`let x = x + 10`) is an error; shadowing works only
  in a nested scope.
- In nested `for` loops over range literals, comparing the two loop variables
  (`if q > r`) fails sema with ``type `?T40` doesn't support ordering comparisons``. It
  looks like an inference gap in loop-variable types.
- `Program.items` includes the injected prelude (`Ordering`, `Hasher` and the core traits) with
  synthetic spans (`Span::is_prelude()`). Anything that walks the items must skip them.
- `SemaContext.expr_types` can hold unresolved type variables, for example for an `if`
  expression, and the unifier's substitution table is not exposed. A backend therefore
  cannot rely on `expr_types` alone for concrete types. The emitter derives types from its
  own lowering and uses `int_literal_types` to catch sized integer literals. A real backend
  probably needs sema to publish resolved types; this is a point to plan around.

Found while building the enum slice and probing for the roadmap, same status (found, not fixed):

- **Closures are an unchecked channel across tiers.** A `@tier(low)` function can call a
  function-typed parameter, and sema accepts a closure that allocates GC structs being passed into
  it (probed, including the allocating case). The call-tier check covers named calls only. This
  breaks the Stage 1A premise that LOW code never starts a collection, which is what makes a LOW
  boundary cost 0 to 0.4 ns and raw pointers into the GC heap safe. A function type needs a tier.
- **Which tiers may call which** (probed, every pair): HIGH may call HIGH, MID and LOW; MID may
  call MID and LOW; LOW may call only LOW. Calling a higher tier is a sema error.
- **No arena value can cross any call:** not to a named function (even with a `[lifetime L]`
  reference parameter), not to a closure, not returned out of its block. Only ints, strings and
  GC values cross. The Stage 1A callback pattern (MID hands a pointer into its arena to a HIGH
  closure) cannot be written in the language as it stands.
- A closure that captures an int or a GC value can be passed to a MID function and called there;
  a MID function can return a GC struct it received as a parameter; a LOW function can return a
  struct (sema accepts it).
- Arena enums with scalar payloads are fine; an arena enum that holds another arena enum is
  rejected by sema, like the struct case.
- The interpreter prints an enum value as `Type.Variant` and compares enums by variant.
- The prelude is `Ordering`, `Hasher` and the core traits (there is no `Option`), so an optional
  arena reference cannot be built from a library type either.

Found while building the struct slice, same status (found, not fixed):

- Structs have **reference semantics** in the interpreter: `let b = a`, passing to a
  function, and `let c = o.i` for a nested struct all alias. `examples/arena_demo/ast_builder.ubl`
  calls `Span` a "plain value type, copy freely", which is not what the interpreter does.
- **The first `examples/arena_demo/ast_builder.ubl` was a sketch that did not parse.**
  `arena` is a reserved word (so `arena: Arena` fails), `arena.alloc`, `alloc_str` and
  `alloc_slice` do not exist in the Ubel front end (only in Rust test code), lambda
  return-type annotations are rejected, and the `from a in args select ...` form was removed.
  It has been rewritten to run (2026-10-23, see "The target program" under Next stages).
- **A recursive arena tree cannot be written in today's language.** Enums cannot take a
  lifetime parameter and cannot carry arena references (`enum E [lifetime L]` is a parse
  error). A user-declared `Maybe<&L Node>` held in an edge struct field is rejected by sema as
  escaping the block. `&L Node?` parses, but `null` is rejected for it (`expected &L Node?,
  found null`), and neither `none` nor an `orelse` operator exists, so there is no way to
  build a leaf. The prelude has no `Option`. Fixed-shape node graphs (one edge struct per
  node shape, mandatory `&L` fields) work, and so do recursive GC enums. This is a language
  gap, not an emitter one: the bootstrap goal needs optional arena references, arena-capable
  enums, or arena-resident lists.
- Passing an arena value as a call argument is rejected by sema even when the parameter is a
  lifetime-generic reference (`fn eval_call [lifetime L] (c: &L CallNode)` called with
  `&call` inside `with arena`). In practice arena nodes have to be built and read inside
  one function; only ints and strings cross a call. Lambdas take their body right after the
  parameter list (`fn(v: int) $"{v}"`) with no return type.
- `mid` (and presumably `low`, `high`) cannot be used as a field name.
- Inside `with arena`, sema leaves some expression types `Unknown` (field access on an arena
  struct and every operator built on it, and assignments). The emitter derives those types
  itself and lets `Unknown` through only for the expression kinds it lowers.
- A plain struct cannot hold an arena struct, even from the same arena: sema reports it as
  escaping the block. `edge struct` with a lifetime and `&a` fields passes sema and the
  interpreter, so that is the intended way for arena values to point at each other; the
  emitter does not support it yet. This is the central pattern of `ast_builder.ubl`.
- A GC struct can be stored into an arena struct field. An arena struct passed as a call
  argument, or stored into a GC struct's field, is rejected by sema (probed with a helper
  that returns its argument and with a store into a parameter). MEMORY_MODEL.md still says
  argument passing is unenforced; either that text is stale or these rejections have another
  cause. Not investigated.
- `Unique.new(P { ... })` passes sema but the interpreter panics on `u.x`
  (`cannot access field 'x' on Unique`), so LOW construction has no interpreter oracle yet.
- Sema treats `int` and `i64`, and `ulong` and `u64`, as different types (`expected i64, found int`), although the
  interpreter gives them one value type and the emitter one representation. An unsuffixed literal too large for `i64`
  is a `u64` and does not go into a `ulong`.
- `let x: T = e` takes its type from `e`; the emitter ignores the annotation (sema has checked it). That is harmless for
  the integers, and it is why `let x: short = 5` needs no refusal.
- Negating an unsigned integer passes sema and panics in the interpreter at run time (`cannot negate u8`).
- `short` and `ushort` are widened by sema and by the interpreter (`70000 as short` is `70000`); `byte` is `i8`, which
  is signed, and `ubyte` is `u8`.
- `Str`, with a capital, is another spelling of `string` for sema and the interpreter; the self-hosted tokenizer writes
  its string fields that way. The emitter accepts it since M2a (example 37).
- Plain `int` arithmetic in the interpreter goes through `f64`: see "Known differences from the interpreter".

### Toolchain notes

The root `Cargo.lock` is format v4, which needs cargo 1.78 or newer, and `miette`'s
`fancy` feature pulls in a `backtrace` release that needs Rust 1.81. To build the core
crate on Rust 1.75 the lockfile had to be marked v3 and `backtrace`, `addr2line`, `gimli`
and `object` pinned to older releases. The lockfile shipped with `crates/ubel_llvm` is
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
| **HIGH tier** (the GC) on wasm | **needs design work** | The PoC collector finds roots by scanning the machine stack and registers conservatively. In WebAssembly the operand stack and locals are not addressable memory, so that cannot see them; only the linear-memory shadow stack is visible. HIGH on wasm needs precise roots, for example an explicit shadow stack that the compiler maintains. This is reasoning from how wasm works, not something tested here. `mmap` and the register spill (`__builtin_unwind_init`) also do not exist there, so the runtime needs a port. MID and LOW are not affected the same way. |

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

- One pointer mask describes up to 64 pointer-words: 512 bytes on 64-bit, 256 on 32-bit. A
  bigger object that holds pointers is an array allocated with `ubel_gc_alloc_array` (a mask per
  element, any total size); a pointer-free object can be any size. Size classes go up to 32 KiB
  and larger objects take runs of whole spans. See `docs/GC_LARGE_OBJECTS.md`.
- HIGH zeroes memory; MID and LOW do not, as an allocator for each tier would.
- 32- and 64-bit targets are supported: sizes and pointer masks are derived from the
  target's data layout, and `run_bench.sh` sets the width of `size_t`.
- A scannable arena is scanned conservatively and in full at every collection (the whole
  of each retired chunk, the bumped part of the current one), so its cost grows with arena
  size.
- A plain arena that holds the only reference to a GC object is a bug, not a supported
  mode. `xit_arena_plain` exists only to demonstrate it.
- A global or static variable is a GC root only if the memory holding it is registered with
  `ubel_gc_add_root` (and unregistered with `ubel_gc_remove_root` before that memory goes away). The emitter
  registers nothing: the only globals it defines are `private unnamed_addr constant`s (string literals and
  static closure objects), which hold no heap pointers. A language feature with mutable globals would need
  the emitter to register them. Nothing here is thread-safe.
- The stack scan is conservative, so a word that merely looks like an address keeps an object alive. On
  64-bit that is rare; on 32-bit a random word lands inside the heap often enough to see (probability
  about heap-in-use / 4 GiB per word). The runtime keeps its own frames free of such words (no
  `setjmp` buffer, clock readings in statics, a wiped zone below `ubel_gc_collect`), but the program's
  own stale slots are out of its hands. `rt_gc_test` scrubs the stack below each test frame for that reason.
- Emitted struct objects are limited to 32 fields so they fit a GC object on 32-bit targets.
- ASan's fake stack (`detect_stack_use_after_return`) can move locals off the real stack,
  where a conservative scan cannot see them. The struct programs pass with it on and off at
  -O0 and -O1, but nothing guarantees that, so treat sanitizer runs of GC programs with care.

## Bugs found while building this

- The first version assumed 8-byte words, so on a 32-bit target (Termux on `armv7a`) the
  collector read the wrong slot, never followed list `next` pointers and freed live
  nodes. Fixed by working in pointer-sized words and deriving layout from the target.
  `--selftest` exists so this class of problem shows up as a failure instead of a
  plausible-looking table.
- The tree workload's depth and repetition arguments were swapped in the harness wrapper.
  Checksums still matched, because all three tiers got the same wrong inputs, which is why
  the allocation counts are also worth checking.
- Emitter: a parameter's register was named `%p.<index>` and a local's stack slot
  `%<name>.<n>`, so a local called `p` could collide with parameter 1. No earlier example
  used a variable `p`, so it stayed hidden until the struct examples. Parameter registers are
  now `%.arg<index>`, a name no identifier can produce.
- Runtime: the conservative stack scan reads poisoned stack words, so a sanitizer build of
  any program that collects reported a stack-buffer-overflow inside the collector. The two
  functions that read the stack are now exempt from ASan instrumentation.
- Runtime, 32-bit only (found 2026-11-04 as an `rt_gc_test` flake, about 0.9% of runs in `large_basic`,
  `array_ptr` and `array_large_children`, always on a 32-bit build, never on 64-bit, so CI did not see it).
  The collector kept dead objects alive through words in its own frames that only look like addresses:
  (1) the `setjmp` buffer in `scan_roots`: glibc stores `esp` and `eip` XORed with a random per-process
  guard and leaves 128 bytes of signal mask uninitialised, so the same random words were read on every
  collection of a run; (2) `t0/t1/t2` in `ubel_gc_collect`: the low word of a nanosecond clock falls inside
  a 5 MB object for about 5 ms out of every 4.3 s, wider than the test's retry window. Fixed: `scan_roots`
  relies on `__builtin_unwind_init` alone, readings go through a `noinline` `stamp()` into statics, and
  `clear_scan_zone()` zeroes the stack below the collector before the scan. Tests: `register_root` (objects
  held only by callee-saved registers) and `clock_alias` (a clock reading equal to a dead object's
  address; deterministic, and it fails on 64-bit too). Mutants caught: losing the register scan, inline
  clock reads, no `clear_scan_zone`. Removing only `__builtin_unwind_init` is NOT caught, because
  `ubel_gc_collect`'s own prologue already pushes the registers; the two layers are redundant by accident.
- A delivery bug: a nested `README.md` in a replacements archive was captured by the
  standing manifest's filename-based override for `README.md` and overwrote the repo's
  root README. The same hazard applies to any nested `Cargo.toml`; the manifest needs an
  explicit entry for `crates/ubel_llvm/Cargo.toml`.

## Next stages

### The target program: `examples/arena_demo/ast_builder.ubl`

Rewritten to run (it parses, passes sema, and the interpreter prints the reference output;
the arithmetic in it was checked by hand). It builds `square(a + b)` two ways and checks
that they agree: a recursive GC enum tree (`Expr`, with `show` and `eval`, in HIGH) and a
fixed-shape arena graph of edge structs (in MID, evaluated through `&L` references, with
the result handed to HIGH as a string by a callback). It also runs a thousand short-lived
arenas. It is the program the native emitter was grown toward, and since A7 **it compiles**: its
native output is identical to `ast_builder.expected` at -O0, -O2 and under ASan+UBSan, at 64 and 32
bits, and it runs in the equivalence loop like every example (A8).

What it needs, by part, in the order I would build it (reasoned, not yet measured):

1. ~~Edge structs with `&L` fields, borrows, lifetime parameters on functions.~~ Done
   (slice 3, see above): a struct value is already a pointer, so `&x` is the same pointer
   and a `&L T` field is a `ptr` field. `checksum` and `build_and_eval` minus the callback
   compile and match the interpreter.
2. ~~Enums with payloads and `match`.~~ Done (slice 4, see above), with the provisional
   uniform-object layout.
3. ~~**Strings as values** (interpolation that builds one, `==`, printing one).~~ Done (A6, see
   "Strings" above), after deciding D3b and after large-object support in the collector
   (`docs/GC_LARGE_OBJECTS.md`: a string can exceed 512 bytes, and the GC path uses the same
   allocator). Then string `+` and ordering (A6b). Strings built in arenas (the D3b sema patch) and a
   string field in an arena object (scannable arenas) are done. String methods: the ones that read a
   string are done (A9, see "String methods" above), and so is `match` on a string (A9b, see "String
   match"), and the methods that build a string are done (A9c); `chars` and `split` stay refused because they
   return lists.
4. ~~**Non-capturing closures** passed to a MID function and called there (`cb(result)`).~~ Done
   (A7, see "Closures" above). This completed `build_and_eval` and `compile_expression`, and with
   it the target program (A8). Capturing closures followed (A7b). Open: captures of strings, structs,
   enums and closures, `mut` captures and captures from more than one level out.

Parallel track that does not block the above: run the slice 3 CI workflows, and the GC
hardening items in `docs/GC_RESEARCH.md` (large objects, global roots and the collect-on-every-allocation
debug mode are done; what is left is stress over more pointer locations as the emitter grows), since items 2 to 4 all allocate more kinds of object.

Decisions the language still owes the emitter: where a MID helper outside `with arena`
allocates (the target program sidesteps it, because sema stops arena values crossing calls
anyway), how LOW allocates, and the recursive-arena-tree gap above. Other open candidates:
publish resolved types from sema, the cross-language baselines, and the benchmark
methodology work.

**The full plan, including the design phase (collector, arenas and allocation types, LOW, the
cross-tier communication matrix and the proof), is in `docs/LLVM_ROADMAP.md`.**

### CI job summaries

Both workflows build their job summary with scripts in `crates/ubel_llvm/scripts/`, so the report
can grow without editing a workflow file (the workflow yml cannot be delivered through the
replacements flow and is committed by hand, once). `ci_info.sh` prints a run-info table: commit,
workflow run, who started it, machine, CPU, OS and runner image, compiler, Rust, and the
workflow inputs. For the Stage 1B workflow, `run_equiv.sh` now also writes `results.tsv` (every
check) and `metrics.tsv` (per compiled program: IR size, functions, allocation sites, one timed
interpreter run and one timed -O2 run, and the collector's statistics, which the runtime prints
to stderr at exit when `UBEL_STATS` is set), and `ci_report_emit.sh` turns them into the summary:
verdict and counts per kind of check, a feature table whose status follows the results of the
examples that prove each row, a table of compiled programs, collector activity, every refusal
with its reason, the target program's status and first blocker, any failures, and the raw log.
The benchmark summary now starts with the same run-info table and ends with the GC self-test
log. The timings in the Stage 1B report are single wall-clock runs on a shared runner and are for
orientation, not measurement.

## Log

- 2026-11-10: M2a: `assert` and `panic`, and the fixed-width integers `i8 i16 i32 u8 u16 u32 u64` ("Integers and `assert`"
  above): literals, every operator, casts, printing, struct fields, enum payloads, closures. New: examples 33 to 36, five
  `fail_` examples and two refusal examples, a regenerated `u_sized_int` (now a refusal for `short`), `ubel_print_u64`,
  `ubel_u64_len`, `ubel_u64_write` and `ubel_panic` in the runtime. Also fixed: shifts are masked and `MIN / -1` is
  defined (both were undefined for `int`). Of the `ok_` fixtures 28 of 176 now compile (they were 4 of 168), and `assert`
  is no longer a first refusal for any of them. **Found:** plain `int` arithmetic in the interpreter goes through `f64`
  (precision lost past 2^53, saturation instead of wrapping; "Known differences"), a core bug for you. Verified on a
  fresh clone: the suite gives 86 passed and 0 failed at 64-bit and at `-m32`; 27 of 28 hand-made mutants were caught
  (the other is an equivalent: LLVM reads `i8 200` and `i8 -56` alike; two real test gaps were found and closed along
  the way); the examples also match the interpreter under clang 15, 18, 19 and 20 and on emulated 32-bit ARM and aarch64 Linux (43 programs each; see `ANDROID_TERMUX.md`). Also: `Str` is accepted as
  another spelling of `string` (example 37), which moves the self-hosted tokenizer's first refusal to its `double`
  field. Not done: floating point (M2b), collections.
- 2026-11-10: M1a: `ubel-llvm build prog.ubl -o prog` (new module `driver`; "Building an executable" above). One
  command from source to a native executable: the IR, the embedded C runtime and one clang invocation. Tests: 5 unit
  tests (the compiler search, with fake compilers) and 12 integration tests (examples against the interpreter, the
  failure paths, temporary directory hygiene, the command line). 17 of 17 hand-made mutants of the driver and the
  command line were caught (one of them, the dropped `-Wno-override-module`, only by the assertion on the command string, because that flag changes no output). Not done: `ubel build` in the `ubel` CLI and the workspace join (M1b), a runtime cache,
  Windows and macOS.
- 2026-11-10: The folder `crates/llvm_poc` is renamed `crates/ubel_llvm` (the owner's choice; section 7 of
  `LLVM_BACKEND_STATUS.md`). One delivery: the directory rename and the path rewritten in 24 files; the two workflows
  were changed separately, by hand. Nothing else changed. Checked on a fresh clone of master 9a46e6c (it already has the
  owner's pipe and array-hint work in core): the emitter builds, `run_equiv.sh` gives 72 passed and 0 failed at 64-bit
  and at `-m32`, the GC test runs 17 tests in each mode, 176 `ok_` fixtures pass sema and 181 `err_` fixtures fail it,
  and the regenerated Unicode files are byte-identical to the committed ones.
- 2026-11-08: A9c follow-up: the string-building methods allocate in the receiver's arena (owner's decision).
  The helpers take an arena parameter; the emitter maps the sema `ArenaId` in a receiver's `&arena` type to the
  open arena pointer. `examples/32_string_arena` (a receiver from an outer arena used in an inner block, 20,000
  iterations in one arena) is identical to the interpreter at -O0, -O2 and under ASan+UBSan at 64-bit and 32-bit
  and under `UBEL_GC_STRESS=1`, with zero GC allocations in the whole program; example 30 now makes 10 fewer GC
  allocations than before (620,314 against 620,324) because its arena function allocates in the arena. Five
  mutants (the innermost arena instead of the receiver's, always a GC copy, the mapping never recorded, the C
  helper ignoring its arena, no 16-byte rounding) are all caught: the first by the nested example, the next two by
  the IR directives, the last two by `rt_gc_test str_build_arena`; the example alone cannot see where a string
  lives, which is why the harness test exists. New: `// expect-ir:` and `// expect-no-ir:` directives in
  `run_equiv.sh`. Not tested: a closure body (the arena filter exists for it; closures that take arena strings are
  refused elsewhere), and a borrowed `&arena string` parameter as receiver (falls back to a GC copy by
  construction, not by a test).
- 2026-11-08: E1 (from `docs/GOOSE_STUDY.md` F2, roadmap D4), payload-free enum variants as static objects,
  provisional. `enum_ctor` returns a private constant `@.enumv.Enum.Variant` (tag, zeroed slots, emitted after
  the enum types) instead of allocating, for every variant without a payload, in unit-only and in mixed enums.
  `examples/31_enum_unit`: payload-free variants in HIGH, `@tier(low)`, `@tier(mid)` outside an arena, inside
  `with arena`, in a struct, a closure and a million-step loop, mixed with payload variants. Identical to
  the interpreter at -O0, -O2 and under ASan+UBSan at 64-bit and 32-bit. Four mutants (tag always 0 in both
  initializer shapes, variant index off by one, allocation restored) are caught; the mixed-enum one first
  survived because `Shape.Dot` is variant 0, and `Token`, whose payload-free variants are 1 and 2, fixed that. Not done: the plain-integer
  form for unit-only enums (a value is still a pointer, to a static object), and the roadmap D4 row still
  says "keep the uniform object". Found: sema rejects an arena-scoped enum value flowing into a struct field
  (`Lamp { on = Light.Green, .. }` inside `with arena` gives "`&arena Light` ... cannot outlive it"), which is
  D6, not E1. Not run: CI on real runners, Termux, macOS.
- 2026-11-07: A9c, the string methods that build a string, provisional. `to_upper`, `to_lower`, `trim`,
  `trim_start`, `trim_end` and `replace` with Rust's full Unicode semantics, through tables generated from the
  standard library (`scripts/gen_unicode.rs`, `runtime/ubel_unicode.h`, Unicode 15.0.0), each result a new GC
  string (rule S4: my choice, not confirmed); refused in LOW and in MID outside an arena. `examples/29_string_unicode`
  (generated) and `examples/30_string_build`; refusal examples `u_string_value` (repurposed: LOW),
  `u_string_build_mid`, `u_string_replace_arg`. Both examples identical to the interpreter at -O0, -O2 and
  under ASan+UBSan at 64-bit and 32-bit, and under `UBEL_GC_STRESS=1` (620,324 and 8,520 collections, one per
  allocation). 26 mutants of the helpers (sigma always medial or always final, no skipping before or after the
  sigma, a lost context side, ASCII and table off-by-ones, a binary search that is off by one or excludes either
  range edge, a pool offset and an expansion length that are wrong, trimming one character only, overlapping
  `replace` counts, a lost end match for the empty needle, a wrong 3-byte mask, a wrong lead byte, a wrong
  length) are caught; one of them (no skipping before the sigma) only by the generated example, so the
  hand-written one now has that case. Found: while the first probe of `Cased` said U+02B0 was not cased, the
  rule skips ignorable characters before looking for a cased one, so the generator now measures what the rule
  can find. Found, in core and not changed: the lexer has no `\u{...}` escape (any character can be written in
  a literal as itself), and a call whose first argument is a parenthesised expression followed by a method
  call fails to parse (`println((a + "y").len())` gives "unexpected `)`", while `println(1, (a + "y").len())`
  and `let n = (a + "y").len()` are fine). Not run: CI on real runners, Termux, macOS; the tables against a
  Rust newer than 1.75 (the regeneration path was tested by faking the version only).
- 2026-11-07: A9b, `match` on a string, provisional. String literal arms, `|` of literals, `_` and a name that
  binds, as a chain of `ubel_str_eq` tests in arm order (first match wins, scrutinee evaluated once), in every
  tier. `examples/28_string_match`; refusal examples `u_enum_string` (repurposed: `match` on an integer, the
  name is historical; it used to test `match` on a string, see 2026-10-31), `u_match_string_guard`,
  `u_match_string_value`. 66 of 66 locally at 64-bit and 32-bit; example 28 under `UBEL_GC_STRESS=1` identical
  at both widths and at -O0 and -O2 (200,229 collections, one per allocation); fixtures unchanged (165 ok, 169
  err). Eight mutants of the lowering (only the first alternative of `|` tested, last arm wins, catch-all
  routed to the end, binding not stored, every literal sent to the first arm, test polarity inverted, a trap
  where nothing matches, the value-form refusal removed) are all caught. Found: when no arm matches the
  interpreter does not fail, it gives a unit value and carries on, and sema does not ask for a catch-all on a
  string `match`; the emitter refuses a value-form `match` without one and lets a statement fall through.
  Found: sema rejects a `match` on an arena string (the literal patterns are typed `&arena string`); not
  changed, see `docs/ARENA_STRINGS.md`. Not run: CI on real runners, Termux, macOS.
- 2026-11-06: A9, the string methods that read a string, provisional. `len`, `is_empty`, `contains`,
  `starts_with` and `ends_with`, in every tier and on arena strings; bytes, like the interpreter (checked:
  `"日本".len()` is 6 in both). `examples/27_string_methods`, `rt_gc_test str_helpers` (16 tests), refusal
  examples `u_string_value` (repurposed to `to_upper`), `u_string_list`, `u_string_method_arg`. 63 of 63
  locally at 64-bit and 32-bit; example 27 under `UBEL_GC_STRESS=1` is identical at both widths (400,212
  collections, one per allocation); fixtures unchanged (165 ok, 169 err). Mutants of the three helpers (first
  byte only, no restart after a partial match, wrong offsets, empty needle, dropped length checks) and of the
  emitted IR (operands swapped at all 62 call sites, results inverted, `is_empty` inverted) are caught; the
  dropped length check in `contains` survives because the loop bound already covers it. The first version of
  the example missed a dropped length check in `starts_with` (the bytes behind the string did not match), which
  is why the harness test has the layout case. Found: the interpreter has no `find` and no `substring`, and
  its `contains` family answers `false` for a non-string argument. Not run: CI on real runners, Termux, macOS.
- 2026-11-04: GC hardening, step 3: global roots, provisional. `ubel_gc_add_root(addr, bytes)` and
  `ubel_gc_remove_root(addr)`: the registered memory is scanned word by word at every collection, like the stack;
  regions are keyed by start address (adding the same start changes the length); a NULL address, under one word,
  or removing an unregistered address aborts. New test `rt_gc_test global_roots` (15 tests): both ends of a region
  (the word one past the end is not scanned, the word before the start is not), a changed length, a start that is
  not word-aligned (only the words that fit inside), nothing registered (the global keeps nothing alive), and 300
  one-word regions removed in an order that exercises the swap-with-last removal; `misuse` has 3 more cases.
  Mutants caught at -O0 and -O2, 64-bit and 32-bit: the last word skipped, removal one past the end, the scan not
  called, a same-start add appended instead of changing the length, an unaligned start. The emitter registers
  nothing, because it defines no mutable globals (checked: its only globals are `private unnamed_addr constant`).
  `run_equiv.sh` at 64-bit and `-m32`: 59 passed, 1 failed each (`20_arena_strings`, waiting for the D3b sema patch).
  Not run: Termux, macOS, CI on real runners.
- 2026-11-04: GC hardening, step 2: collect-on-every-allocation mode, provisional. `UBEL_GC_STRESS=N`,
  `ubel_gc_set_stress`, `GC_STRESS=N` for `run_equiv.sh`. New test `rt_gc_test stress_mode` (14 tests): in stress
  mode it builds objects reachable from one kind of place only (struct fields, a pointer array, an array on the
  large-object path, a scannable arena with retired chunks, enum-like and closure-like objects) while each new
  object is held only by the builder's frame, checks a collection happened before every allocation, that every
  object is intact, and that live bytes equal the bytes allocated (nothing in it is garbage); then churns, then
  drops everything and expects 0. Mutants: a trigger that never collects is caught; skipping the arena roots is
  caught (crash); a scan that starts above the collector's own frames is NOT caught by this test (the
  `register_root` test catches it). Whole example suite under `GC_STRESS=1`: 64-bit 59 passed and 1 failed, `-m32`
  59 passed and 1 failed, each time with `20_arena_strings` (waits for the D3b sema patch) the failure; every
  other example is identical to the interpreter at -O0, -O2 and ASan+UBSan with a collection before each of
  its allocations (for example 610,225 collections in `24_string_ops`, 1,800,331 in `25_arena_scan`). It found no
  collector bug in the examples. Not run: Termux, macOS, CI on real runners; `-m32` without stress after the
  script change.
- 2026-11-04: GC hardening, step 1, provisional. 32-bit `rt_gc_test` flake traced and fixed (see Bugs found):
  `setjmp` buffer and clock readings no longer scanned as roots, plus two new tests, `register_root` and
  `clock_alias` (13 tests). After the fix at 32-bit -O0: `large_basic` 0 of 3000 runs failed (about 0.9% before),
  `array_ptr` and `array_large_children` 0 of 1500 each, `classes` and `reuse` 0 of 800, `interior` 0 of 400.
  `run_equiv.sh` at 64-bit and `-m32`: 59 passed, 1 failed each time, the failure being `20_arena_strings`,
  which waits for the D3b sema patch. Not run: Termux, macOS, CI on real runners.
- 2026-11-02: D2 closed, provisional. Sema `TIER-017`: a LOW function may call a closure only if it is an
  immutable local bound straight to a lambda in that function; the emitter applies the same rule, so a LOW
  function can now create and call closures (`26_closure_low`). `u_closure_low` now tests a capturing
  closure in LOW. No other fixture or example changed behaviour (370 files checked).
- 2026-11-01: scannable arenas, provisional. An arena whose objects have pointer fields is created
  scannable, so strings, structs, enums and closures can be stored and captured in arena objects; the
  runtime scans only written bytes. Lifted: the string-field and the int/bool-only capture refusals.
  `25_arena_scan`, `rt_gc_test arena_scan`; the refusal examples `u_enum_string` and
  `u_closure_capture_string` now test `match` on a string and comparing closures (names are historical).
- 2026-10-31: A6b (string `+` and ordering), provisional. `a + b` is a GC copy wherever it is written, `<`
  `<=` `>` `>=` compare bytes. `examples/24_string_ops` (empty strings, prefixes, non-ASCII bytes, `+=`,
  a 100,000-byte string through the large-object path, strings kept alive only by a struct field and
  an enum payload across 200,000 rounds); `examples/20_arena_strings` needs the D3b sema patch, re-issued
  because the closure patch was committed over it before it ran. 57 of 57 without the patch; mutants of
  the comparison sign, the prefix tie-break, the copy offset and the LOW gate are caught.
- 2026-10-31: A7b (closures that capture), provisional, with decisions D2 (a closure's tier is that of the
  function it is created in) and D5 (the object lives in the arena of its block). Ints and bools only,
  copied at creation. Found and fixed in sema: arena closures could not be called, and the error
  panicked the checker. 55 of 55 locally; mutants of the capture code are caught.
- 2026-10-31: A7 (closures that capture nothing) and A8 (the target program compiles), provisional.
  `fn(...)` types and `fn(v: int) ...` values, static closure objects, indirect calls. The target
  program's native output is identical to `ast_builder.expected`, and it now runs in the equivalence
  loop. 49 of 49 locally at 64-bit and 32-bit; three mutants of the closure lowering are caught.
  Open: capturing closures (D5) and the tier of a closure (D2).
- 2026-10-31: A6 (strings as values), provisional. `string` as a type, static literals, `$"..."` that
  builds a string by site, `==` and `!=`, printing a string value. 44 of 44 locally at 64-bit and
  32-bit; mutants of the pointer mask and of two runtime helpers are caught. The target program
  now stops at its closure parameter (A7). Strings built in an arena were tested with the D3b sema
  patch applied (45 of 45) and are refused without it.
- 2026-10-25: Stage 1B slice 4: enums with tuple payloads and `match` (uniform heap object per
  value, per-variant pointer masks, `switch` on the tag), 36 of 36 locally at 64-bit and 32-bit,
  not yet on CI. CI summaries moved into scripts and expanded (run info, feature table,
  per-program metrics, collector activity, refusals, target status). New `docs/LLVM_ROADMAP.md`
  with the draft cross-tier matrix. Probing for it found that closures bypass the tier call
  lattice, and that no arena value can cross a call.
- 2026-10-24: Stage 1B slice 3: edge structs and struct references (`&x`, `&L T` fields,
  `[lifetime L]` on functions), 27 of 27 locally at 64-bit and 32-bit, not yet on CI. CI
  confirmed slices 1 and 2 (run #2: 21 of 21; run #3: 22 of 22, both runners). Benchmark
  run #5 recorded: aarch64 identical to run #4, x86 LOW moved more than HIGH or MID,
  unexplained. The target program's first refusal is now `enum`.
- 2026-10-23: Stage 1B slice 2: plain structs (GC allocation in HIGH, arena bump allocation
  inside `with arena`; MID-outside-arena and LOW literals refused). 21 of 21 locally at
  64-bit and 32-bit, not yet on CI. Fixed a parameter-name collision in the emitter and made
  the runtime's stack scan ASan-exempt. Wasm demo now skips allocating programs. Front-end
  findings recorded. `examples/arena_demo/ast_builder.ubl` (a sketch that did not parse)
  rewritten as a runnable program and made the emitter's target; roadmap to it recorded.
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
