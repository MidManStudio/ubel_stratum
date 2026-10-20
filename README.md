# llvm_poc: Stage 0

A small, isolated proof of concept for Ubel's LLVM backend. Nothing here
depends on the compiler crates, and nothing outside `crates/llvm_poc/` is
touched, so it cannot interfere with work happening in `core`, `sema` or
`rd_parser`.

## What it answers

What does each memory tier cost when the code is native LLVM IR? The same
three workloads are written three times in LLVM IR, once per tier, in the shape
a tier-aware backend would emit. Every tier must produce the same checksum.

| tier | allocation strategy in this PoC |
|---|---|
| HIGH | calls into a stop-the-world, non-moving mark-sweep GC (`runtime/ubel_rt.c`) |
| MID | `create` / inlined bump-pointer allocation / `destroy` on an arena |
| LOW | direct `malloc` / `free`, freed explicitly |

Workloads: `churn` (temporary lists that die every frame), `churn+live` (the
same churn while a 6.4 MB list stays live, which is what makes GC marking
cost visible), `tree` (AST-style build, walk, drop), and `arith` (no
allocation, a sanity baseline).

## Running it

```
bash crates/llvm_poc/scripts/run_bench.sh --selftest   # correctness first
bash crates/llvm_poc/scripts/run_bench.sh              # full run, ~5 timed runs per tier
bash crates/llvm_poc/scripts/run_bench.sh --iters 9 --only tree
```

Needs clang 15 or newer (opaque pointers). On Termux: `pkg install clang`.
`EXTRA_FLAGS=-m32` and `CLANG=clang-18` are supported.

**Run `--selftest` once on every new target before trusting its timings.** It
forces a collection every 16 KiB and fails unless all tiers agree, the
collector actually ran, and it kept a known live list alive. Without it, a run
can look healthy while the GC was never exercised (the report prints a note
when HIGH did zero collections).

On GitHub: Actions, "Ubel LLVM PoC", Run workflow. It runs the self-test and
the benchmark on x86_64 and aarch64 runners and writes the tables to the job
summary. The optimized IR (`tiers.opt.ll`) is uploaded as an artifact.

## How to read the numbers

- Compare tiers **within one run on one machine**. Object sizes follow the
  target's data layout (a `Node` is 32 bytes on 64-bit and 28 on 32-bit), so
  absolute times are not comparable across targets.
- Small `--scale` values are smoke tests. At `--scale 0.02` the default
  collector trigger (4 MiB) is never reached, so HIGH times exclude all GC work.
- Shared CI runners are noisy. Min and median of several runs are reported;
  trust big ratios, not small ones.
- Nothing here has been tuned. The MID arena keeps a small cache of 64 KiB
  chunks (as a real arena runtime would); the GC is a first cut.

## What Stage 0 does not model

- **No concurrent GC.** Collection is stop-the-world, so "max pause" is the
  whole collection time. It says nothing about Go-style low-pause behaviour.
  That needs write barriers, safepoints and precise stack maps in the emitted IR.
- **Roots are conservative.** The stack and registers are scanned
  conservatively; heap objects are scanned precisely from a pointer bitmap.
  Globals are not roots.
- **Small objects only**: up to 64 pointer-words (512 bytes on 64-bit, 256 on
  32-bit). Larger requests abort.
- **HIGH zeroes memory, MID and LOW do not**, as an allocator for each tier
  would. This costs HIGH a little.
- **Pointer width**: sizes and pointer masks are derived from the target's
  data layout in `bench/tiers.ll`, and `run_bench.sh` sets the width of
  `size_t`. 32- and 64-bit targets are supported.

## Layout

```
runtime/ubel_rt.h     public API of the three allocation strategies
runtime/ubel_rt.c     GC (size-class spans, tri-colour mark-sweep) and arenas
bench/tiers.ll        the workloads, hand-written LLVM IR
harness/bench_main.c  timing, checksum checks, markdown report, --require-gc
scripts/run_bench.sh  build and run (also --selftest)
```

## Next stages

1. Emit textual IR from `Program` + `SemaContext` for a tiny subset
   (arithmetic, locals, `if`/`while`, calls). Textual IR keeps libLLVM out of
   the Cargo build.
2. Tier-aware lowering: `with arena` to arena create/bump/destroy, LOW
   allocation to malloc/free, HIGH to the GC runtime.
3. Concurrent marking, write barriers and precise roots, after the other
   collectors (Go, Java, Swift, C#) have been compared.
