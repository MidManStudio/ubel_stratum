# LLVM proof of concept

Status: **Stage 0 done, measured on CI. Stage 1 (emit IR from Ubel source) not started.**

Everything lives in `crates/llvm_poc/` and is isolated: it does not depend on
the compiler crates, so it cannot interfere with work in `core`, `sema` or
`rd_parser`.

## Why this exists

The interpreter cannot say anything about the memory tiers. It runs HIGH, MID
and LOW on the same reference-counted values, and `with arena` blocks are
validated but never allocate. Before building an IR emitter, Stage 0 measures
what each tier costs when it is real native code.

## Stage 0: what it is

The same three workloads are written three times in hand-written LLVM IR, once
per tier, in the shape a tier-aware backend would emit. Every tier must produce
the same checksum.

| tier | allocation strategy in this PoC |
|---|---|
| HIGH | calls into a stop-the-world, non-moving mark-sweep GC (`runtime/ubel_rt.c`) |
| MID | arena create, inlined bump-pointer allocation, destroy |
| LOW | direct `malloc` / `free`, freed explicitly |

Workloads: `churn` (temporary lists that die every frame), `churn+live` (the
same churn while a 6.4 MB list stays live, which makes GC marking cost
visible), `tree` (AST-style build, walk, drop), `arith` (no allocation, a
sanity baseline).

```
crates/llvm_poc/runtime/ubel_rt.h     API of the three allocation strategies
crates/llvm_poc/runtime/ubel_rt.c     GC (size-class spans, tri-colour mark-sweep) and arenas
crates/llvm_poc/bench/tiers.ll        the workloads, hand-written LLVM IR
crates/llvm_poc/harness/bench_main.c  timing, checksum checks, markdown report, --require-gc
crates/llvm_poc/scripts/run_bench.sh  build and run (also --selftest)
```

## Running it

```
bash crates/llvm_poc/scripts/run_bench.sh --selftest   # correctness first
bash crates/llvm_poc/scripts/run_bench.sh              # full run
bash crates/llvm_poc/scripts/run_bench.sh --iters 9 --only tree
```

Needs clang 15 or newer (opaque pointers). `EXTRA_FLAGS=-m32` and
`CLANG=clang-18` are supported. On Termux: `pkg install clang`.

**Run `--selftest` once on every new target before trusting its timings.** It
forces a collection every 16 KiB and fails unless all tiers agree, the
collector really ran, and it kept a known live list alive. The report also
prints a note when HIGH did zero collections.

On GitHub: Actions, "Ubel LLVM PoC", Run workflow. It runs the self-test and
the benchmark on x86_64 and aarch64 runners and writes tables to the job
summary. The optimized IR (`tiers.opt.ll`) is uploaded as an artifact.

## Results

### Run #1: GitHub runners, clang 18.1.3, 7 timed runs per tier, scale 1

Minimum time in ms (the median was within about 3% of the minimum everywhere).
Ratio is tier time divided by HIGH time, so below 1.00 is faster than HIGH.

| runner | benchmark | HIGH (GC) | MID (arena) | LOW (malloc/free) | MID / HIGH | LOW / HIGH |
|---|---|---:|---:|---:|---:|---:|
| x86_64 | churn | 48.70 | 10.45 | 58.99 | 0.21 | 1.21 |
| x86_64 | churn+live | 115.13 | 11.02 | 62.15 | 0.10 | 0.54 |
| x86_64 | tree | 47.01 | 17.36 | 72.53 | 0.37 | 1.54 |
| aarch64 | churn | 45.17 | 15.63 | 63.93 | 0.35 | 1.42 |
| aarch64 | churn+live | 98.42 | 16.56 | 67.49 | 0.17 | 0.69 |
| aarch64 | tree | 43.03 | 15.43 | 73.31 | 0.36 | 1.70 |

Time per allocation (run time divided by allocation count, so it includes
initialising and walking the node), in ns:

| runner | benchmark | allocations | HIGH | MID | LOW |
|---|---|---:|---:|---:|---:|
| x86_64 | churn | 4,000,000 | 12.2 | 2.6 | 14.7 |
| x86_64 | churn+live | 4,200,000 | 27.4 | 2.6 | 14.8 |
| x86_64 | tree | 3,276,700 | 14.3 | 5.3 | 22.1 |
| aarch64 | churn | 4,000,000 | 11.3 | 3.9 | 16.0 |
| aarch64 | churn+live | 4,200,000 | 23.4 | 3.9 | 16.1 |
| aarch64 | tree | 3,276,700 | 13.1 | 4.7 | 22.4 |

GC activity in the fastest HIGH run:

| runner | benchmark | collections | GC time (ms) | share of run | max pause (ms) | live at last GC |
|---|---|---:|---:|---:|---:|---:|
| x86_64 | churn | 30 | 0.29 | 0.6% | 0.017 | 5 KiB |
| x86_64 | churn+live | 21 | 64.22 | 55.8% | 3.121 | 6265 KiB |
| x86_64 | tree | 24 | 0.14 | 0.3% | 0.017 | 7 KiB |
| aarch64 | churn | 30 | 0.25 | 0.6% | 0.014 | 5 KiB |
| aarch64 | churn+live | 21 | 50.96 | 51.8% | 2.476 | 6265 KiB |
| aarch64 | tree | 24 | 0.17 | 0.4% | 0.008 | 3 KiB |

Checksums were equal across tiers and runs, and the self-test passed, on both
runners.

### What the numbers say

- **MID is the clear win for bulk temporary data.** 2.7x to 10x faster than
  HIGH and 4x to 6x faster than LOW on every workload, on both architectures.
  Allocation is a few ns and freeing the whole arena costs almost nothing.
- **HIGH versus LOW depends on how much data is live.** With little live data
  (`churn`, `tree`) HIGH beats `malloc`/`free` by 1.2x to 1.7x: garbage costs
  almost nothing to collect (GC is under 1% of the run) while `free` pays per
  object. With a 6.4 MB live set (`churn+live`) the order flips and `malloc`/`free`
  is 1.5x to 1.9x faster than HIGH.
- **HIGH's cost has two parts.** The allocation path itself is about 11 to 14 ns
  per object against about 3 ns for a bump allocation. On top of that, marking
  the live set costs about 12 to 15 ns per live list node on every collection,
  and with a 6.4 MB live set that was 52% to 56% of the run. Collections are
  paced by the live size (GOGC=100 style), so the cost scales with live data
  times the number of collections.
- **Pause.** One stop-the-world collection of the 6.4 MB live list took about
  2.5 to 3.1 ms. A linked list is the worst case for tracing (every step is a
  dependent load), so that is a pessimistic per-node figure, but it does grow
  with the live heap.

### What they do not say

- No concurrency. A concurrent collector would move marking off the mutator
  thread, shrinking pauses and wall time, at the price of write barriers that
  are not measured here. Stage 0 says nothing about Go-style pause behaviour.
- One thread, one object size (32 bytes), and a list-shaped live set. Trees
  and arrays mark differently.
- LOW is glibc `malloc`. A pool or a different allocator would change LOW.
- Roots are conservative and globals are not roots. The GC is a first cut and
  has not been tuned; its allocation fast path in particular has room to improve.
- Two shared CI runners. Absolute times are not comparable between them (the
  `arith` baseline differs by about 2x); compare tiers within one runner.
- Object sizes follow the target's layout (a `Node` is 32 bytes on 64-bit and
  28 on 32-bit), so absolute times are not comparable across targets either.

## Limits of the Stage 0 runtime

- Objects up to 64 pointer-words: 512 bytes on 64-bit, 256 on 32-bit. Larger
  requests abort.
- HIGH zeroes memory; MID and LOW do not, as an allocator for each tier would.
- 32- and 64-bit targets are supported: sizes and pointer masks are derived
  from the target's data layout, and `run_bench.sh` sets the width of `size_t`.

## Bugs found while building this

- The first version assumed 8-byte words, so on a 32-bit target (Termux on
  `armv7a`) the collector read the wrong slot, never followed list `next`
  pointers and freed live nodes. Fixed by working in pointer-sized words and
  deriving layout from the target. `--selftest` exists so this class of
  problem shows up as a failure instead of a plausible-looking table.
- The tree workload's depth and repetition arguments were swapped in the
  harness wrapper. Checksums still matched, because all three tiers got the
  same wrong inputs, which is why the allocation counts are also worth checking.

## Next stages

1. **Stage 1:** emit textual IR from `Program` + `SemaContext` for a tiny subset
   (arithmetic, locals, `if`/`while`, calls). Textual IR keeps libLLVM out of
   the Cargo build. The core crate currently exposes `lexer`, `error_management`,
   `ast`, `builtins`, `sema` and `interpreter` as public modules.
2. **Stage 2:** tier-aware lowering: `with arena` to arena create/bump/destroy,
   LOW allocation to malloc/free, HIGH to the GC runtime.
3. **Later:** concurrent marking, write barriers and precise roots, after the
   Go, Java, Swift and C# collectors have been compared.

## Log

- 2026-10-20: Stage 0 delivered; self-test and benchmark run on x86_64 and
  aarch64 runners (run #1, above).
