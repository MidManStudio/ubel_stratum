# HIGH-tier GC research notes

Status: **working notes, provisional.** Nothing here is a design decision. The HIGH tier
starts from a Go-style collector in the PoC, but the intent is to study the Go, Java,
Swift and C# approaches properly and take what fits Ubel's three-tier model. These notes
collect what was checked so far, so later design work does not inherit mistakes.

The material below started as summaries written by AI assistants. Claims were sorted by
checking them against primary sources where that was possible. Each is marked:

- **checked**: confirmed against a source linked below;
- **wrong**: contradicted by a source;
- **not re-checked**: plausible and common knowledge, but not verified in this pass, so
  treat it as a lead and not a fact.

## Claims that were checked

| claim | verdict | source |
|---|---|---|
| Go uses a concurrent mark-and-sweep collector with a write barrier during marking | **checked** | Go runtime `mgc.go` |
| Go's collection cycle has two stop-the-world phases (sweep termination and mark termination), with marking and sweeping concurrent | **checked** | Go runtime `mgc.go` |
| Go's collector does not move heap objects | **checked** | Go runtime source (`mgc.go`, `mbarrier.go`) |
| Go paces collections from the live heap with `GOGC`, with a minimum heap trigger of about 4 MB at the default setting | **checked**; the PoC's 4 MiB floor and GOGC=100 pacing mirror this | Go runtime `mgc.go` |
| G1 uses write barriers: a snapshot-at-the-beginning pre-barrier while marking and a card-marking post-barrier feeding remembered sets | **checked** | OpenJDK `g1BarrierSet.hpp`; "Write Barriers in Garbage First" slides |
| .NET's background (concurrent) GC applies to generation 2 in both workstation and server configurations | **checked** | Microsoft .NET GC documentation |
| .NET's large object heap is not compacted by default (compaction can be requested) | **checked** | `GCLargeObjectHeapCompactionMode` documentation |
| Swift heap objects are created by the runtime's `swift_allocObject` and are reference counted with atomic retain/release by default | **checked** in outline | Swift runtime `HeapObject.cpp`; "The Swift Runtime: Heap Objects" |

## Claims that were wrong in the pasted material

| pasted claim | what the sources say |
|---|---|
| "G1/ZGC: no generational write barrier needed for young gen" | **wrong for G1.** G1 has both a pre-write and a post-write barrier, as above. ZGC is a different design and was not re-checked here, so the claim about it is unverified. |
| "Swift allocates with a bump pointer" | **not supported.** Nothing found shows a bump allocator; Swift heap objects come from `swift_allocObject`, which as far as the runtime source and write-ups show goes through the platform allocator. Heap allocation is described as notably more expensive than stack allocation. |
| ".NET Gen2 can run in the background on server GC" (implying server only) | **imprecise.** Background GC is available in both workstation and server configurations. |
| "Go: bump-pointer allocation to per-thread areas" | **imprecise, not re-checked.** From memory, Go allocates small objects from size-class spans through per-P caches, using free-slot bitmaps, which is the design the PoC copied. It is not a plain bump pointer. |
| "Go pauses are independent of heap size" (as an absolute) | **overstated.** The two stop-the-world phases are short and Go's pauses are usually well under a millisecond, but concurrent marking still costs CPU and mutator assists, and nothing is guaranteed. |

## Claims not re-checked

Treat these as leads to verify before relying on them: G1's default pause target
(believed to be 200 ms) and its remembered-set overhead; ZGC's barrier design and
sub-millisecond pauses; the exact generation layout and promotion rules of the .NET GC;
Swift's ARC optimization behaviour; and any specific latency numbers.

## What each runtime can teach Ubel

Kept short and at the level the checks support.

- **Go:** concurrent marking, pacing from the live heap, allocation assists, a collector
  that never moves objects (so pointers handed to C or to LOW code stay valid), and good
  observability. The PoC collector is a stop-the-world simplification of this.
- **Java (G1, ZGC, Parallel):** several collectors for different latency and throughput
  goals; generational and region-based designs; compaction. The cost is that moving
  objects means every reference must be found and updated.
- **.NET:** generations and a cheap allocation path, background collection of the old
  generation, a card table to find old-to-young references, and pinning for native code.
- **Swift:** compiler-driven lifetimes, borrowing and ownership transfer, deterministic
  destruction. Reference counting does not collect cycles and pays for atomic operations,
  so it is a source of ideas for the compiler's ownership analysis more than a drop-in
  replacement for tracing.

## Constraints specific to Ubel's current design

These follow from what the PoC and the docs actually do, and matter more than any single
algorithm.

- **The collector is non-moving and scans the stack conservatively.** A moving or
  compacting collector cannot relocate an object that a conservatively scanned stack word
  might point to, so moving would need pinning of anything reachable from the stack, or
  precise stack maps from the compiler. Generational collection does not require moving
  (non-moving generational designs exist), so those are separable questions.
- **Cross-tier references decide the root model.** Stage 1A showed that a GC pointer held
  only in plain arena memory is invisible to the collector. Whatever collector is chosen
  must either see arena contents that can hold GC references, or the language must keep
  such references out of arenas. Candidates (not mutually exclusive): scannable arenas
  (implemented in the PoC), compiler-chosen arena kinds from static analysis of stored
  types, explicit root registration, handles, and ownership restrictions.
- **The other direction is already guarded.** Sema rejects arena references escaping into
  longer-lived places, so the arena-to-GC direction is a type-checking question already.
- **LOW code is a GC-free region by construction.** LOW cannot call HIGH or MID, so no
  collection can start while LOW runs. That is useful for any concurrent design.
- **Write-barrier sites exist already.** Every pointer store into a GC object (child links,
  closure creation, list links) would need a barrier in a concurrent or generational
  collector, as would GC pointers stored into scannable arenas.
- **WebAssembly changes the root problem.** Conservative stack scanning relies on reading the
  machine stack, and in WebAssembly the operand stack and locals are not addressable memory;
  only the shadow stack in linear memory is. A HIGH tier that runs in a browser therefore
  needs precise roots (an explicit shadow stack maintained by the compiler, or compiler
  generated root maps). This follows from how wasm works and was not tested; it is one more
  reason to treat precise roots as a design goal and not only an optimization.
- **Native pointers.** If the LLVM backend calls into C (the Mid Engine packet codec, for
  example), non-moving makes that simple. A moving collector would need handles or
  pinning at that boundary.

## Open questions

1. Where can GC references live: stacks, registers, globals, heap objects, closures,
   arenas, native code? How does the collector find each? (Globals are not roots in the
   PoC today.)
2. Which stores need barriers, and what do they cost in generated code?
3. Can a pointer to a HIGH object be stored in MID or LOW memory, and under which rules?
4. Can a MID object outlive its arena through a callback, closure or returned value?
5. What does native code do with a pointer it keeps after a call returns?
6. Do Ubel's real workloads have the young-object mortality that would justify a
   generational collector? This needs measuring on real programs, not assuming.
7. What latency budget matters: a game frame, a server request, a batch job? The goals
   conflict, and the collector should be configurable rather than tuned for an imaginary
   universal workload.

## A suggested order for the research

Provisional, and to be decided together.

1. Settle the root and cross-tier contract (questions 1 to 5) before changing algorithms.
2. Make the current collector trustworthy: global roots, large objects, stress tests that
   collect at awkward points, and a debug mode that collects on every allocation.
3. Measure properly: allocation fast path, pause distributions (median, p95, p99, worst),
   memory overhead, and allocation lifetimes on real programs.
4. Only then evaluate generational and concurrent designs against measured needs.
5. Treat moving or compacting collection last, after the stack-map and pinning story is
   defined.

## Sources

Read for the checks above:

- Go runtime GC: https://go.googlesource.com/go/+/HEAD/src/runtime/mgc.go
- Go runtime write barrier: https://go.googlesource.com/go/+/go1.9/src/runtime/mbarrier.go
- G1 write barriers (slides): https://www.jfokus.se/jfokus17/preso/Write-Barriers-in-Garbage-First-Garbage-Collector.pdf
- OpenJDK G1 barrier set: https://builds.shipilev.net/patch-openjdk-leyden-premain/src/hotspot/share/gc/g1/g1BarrierSet.hpp.html
- .NET `GCLargeObjectHeapCompactionMode`: https://learn.microsoft.com/ja-jp/dotnet/api/system.runtime.gclargeobjectheapcompactionmode
- The Swift Runtime, heap objects: https://belkadan.com/blog/2020/08/Swift-Runtime-Heap-Objects/

Not read in this pass, suggested starting points: the Go GC guide (go.dev/doc/gc-guide),
Microsoft's .NET GC fundamentals, Oracle's HotSpot GC tuning guide, the Swift book's chapter
on automatic reference counting, and *The Garbage Collection Handbook* (Jones, Hosking,
Moss) for the general theory.
