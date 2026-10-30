# LLVM proof of concept

Status: **Stage 0 and Stage 1A measured on CI (five runs). Stage 1B (emit IR from Ubel source): slices 1 and 2 (scalars, plain structs with per-tier allocation) verified on CI on x86_64 and aarch64; slice 3 (edge structs and references) is on master and slice 4 (enums and `match`) is in this change, both verified locally only so far. The plan from here is in `docs/LLVM_ROADMAP.md`. A browser-playground feasibility check is recorded below.**

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
layout:** every enum value is an object, payload-free variants included: an 8-byte tag (the
variant's position in the declaration), then as many 8-byte payload slots as the widest variant has,
so all values of one enum have one size. A pointer payload sits at the start of its slot. The
collector mask depends on the variant (only some slots hold pointers), so it is computed per
variant from the target's layout, like a struct's. Allocation sites follow the struct rule: inside
`with arena` bump allocation, in HIGH the GC heap, elsewhere refused. A variant that carries
a discriminant is treated as payload-free (sema validates the value and nothing reads it back).
`match` lowers to a `switch` on the tag; a tag no arm covers traps, so a hole in sema's
exhaustiveness check cannot become undefined behaviour. Patterns supported: `Enum.Variant(a, _)`,
bare payload-free variant names, `_`, and a plain name that binds the whole value. Refused by
name: struct-style variants and patterns, `where` guards, nested patterns, `mut` bindings,
generic enums, string payloads (until strings exist), printing an enum value and `==` on enums
(the interpreter prints `C.Red` and compares by variant; the emitter does neither yet). At most 31
payload fields per variant. An arm such as `=> return n,` is parsed as a one-statement block (the
parser treats `return`, `break`, `continue` and `fail` as statements), so arm bodies are lowered as
blocks or expressions, and a `match` used as a value skips the arms that leave the function or loop.

Everything else is **refused with a located "not supported yet" error** instead of being
guessed at: strings as values, collections, closures, generics, struct methods,
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

An arena struct that holds a pointer to a GC struct is only safe while something else keeps
that GC object alive. Today that is always the case: in this emitter a MID function cannot
build GC objects (its literals outside an arena are refused), so the only GC pointers it can
store came in as parameters, and the caller's frame still holds them. This stops being true once closures or callbacks can return GC objects
into MID code (the Stage 1A hidden-root hazard); arenas are plain here, not scannable.

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
and the run reports whether the emitter compiles it yet (22 checks in all). Corrupting the
expected file turns the run red. CI run #3 at 0d5079d: 22 of 22 on both runners.

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
program's first refusal is now its `string` payload (`Var(string)`).

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
- A delivery bug: a nested `README.md` in a replacements archive was captured by the
  standing manifest's filename-based override for `README.md` and overwrote the repo's
  root README. The same hazard applies to any nested `Cargo.toml`; the manifest needs an
  explicit entry for `crates/llvm_poc/Cargo.toml`.

## Next stages

### The target program: `examples/arena_demo/ast_builder.ubl`

Rewritten to run (it parses, passes sema, and the interpreter prints the reference output;
the arithmetic in it was checked by hand). It builds `square(a + b)` two ways and checks
that they agree: a recursive GC enum tree (`Expr`, with `show` and `eval`, in HIGH) and a
fixed-shape arena graph of edge structs (in MID, evaluated through `&L` references, with
the result handed to HIGH as a string by a callback). It also runs a thousand short-lived
arenas. It is the program the native emitter is grown toward, and it is **not compilable
yet**: the emitter now stops at its first `enum`.

What it needs, by part, in the order I would build it (reasoned, not yet measured):

1. ~~Edge structs with `&L` fields, borrows, lifetime parameters on functions.~~ Done
   (slice 3, see above): a struct value is already a pointer, so `&x` is the same pointer
   and a `&L T` field is a `ptr` field. `checksum` and `build_and_eval` minus the callback
   compile and match the interpreter.
2. ~~Enums with payloads and `match`.~~ Done (slice 4, see above), with the provisional
   uniform-object layout.
3. **Strings as values** (interpolation that builds one, `==`, printing one). Needs a string
   representation decision, and **large-object support in the collector first**: a string
   can exceed the 512-byte small-object limit and the runtime aborts on that today.
4. **Non-capturing closures** passed to a MID function and called there (`cb(result)`),
   using the Stage 1A closure proposal (GC object, code pointer in the first word) with zero
   captures at first. This completes `build_and_eval` and `compile_expression`.

Parallel track that does not block the above: run the slice 3 CI workflows, and the GC
hardening items in `docs/GC_RESEARCH.md` (global roots, large objects, a collect-on-every-
allocation debug mode), since items 2 to 4 all allocate more kinds of object.

Decisions the language still owes the emitter: where a MID helper outside `with arena`
allocates (the target program sidesteps it, because sema stops arena values crossing calls
anyway), how LOW allocates, and the recursive-arena-tree gap above. Other open candidates:
publish resolved types from sema, the cross-language baselines, and the benchmark
methodology work.

**The full plan, including the design phase (collector, arenas and allocation types, LOW, the
cross-tier communication matrix and the proof), is in `docs/LLVM_ROADMAP.md`.**

### CI job summaries

Both workflows build their job summary with scripts in `crates/llvm_poc/scripts/`, so the report
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
