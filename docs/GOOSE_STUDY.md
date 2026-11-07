# Goose: what it does, and what is worth taking from it

Status: **provisional notes from reading, not from running.** Written 2026-11-06 from a clone of
https://github.com/aardappel/goose (commit `7b90f91d`). Nothing here was built or measured by us; every
number is Goose's own, and every "suggests" below is an idea for the owner to accept or reject, not a
decision. Findings are tied to our open items (D1 v2, D4, D6, D7, the stable-address and LOW-slice questions,
the OOM policy, A10) so they can be picked up one at a time.

## 0. Summary

- **Backend.** Goose emits one C file and hands it to the system C compiler (`-O2` by default). `goose run`
  uses a bundled TinyCC in-process, which its own docs call "not for speed". It has no LLVM backend of its own;
  under clang it reaches LLVM through the C. Ours emits LLVM IR directly.
- **Memory model.** No heap, no GC, no destructors. The compiler assigns a fixed number of "data stacks";
  each is a huge reserved address range with a bump pointer, one growable value on top, scope exit restores
  the pointer. Lifetimes are checked at compile time by tracking, for every reference, the set of scopes it
  may point into, with no annotations.
- **The "3x faster than C" headline** is a geometric mean over 16 author-written benchmarks, against
  *idiomatic C++* (`unique_ptr`, `unordered_map`, `variant`), not C: 3.51x (MSVC) and 3.54x (clang). Against
  hand-written arena C++ it is 1.24x / 1.26x, against the best safe Rust 1.13x / 1.19x.
- **Where the gap comes from**, by the author's own account: compact inline layouts (variable-size nodes,
  inline text, 1/2/4-byte links), in-place construction, and no teardown. The bump pointer by itself is
  the same trick a hand-written arena in C++ or Rust already uses.
- **Most useful to us:** (F1) how they make "return a reference into the caller's region" sound, which is our
  D1 v2; (F2) their enum layout, which bears on D4 now that strings and closures exist; (F8) their benchmark
  caveats, which are the checklist our A10 evidence should be held to; (F9) their prior-art survey, which
  supports our arena-plus-GC tier shape. The reserved-address-range arena (F3) is interesting but untested
  for us and has costs on 32-bit and on Android.
- **Not worth copying now:** the recursion restriction, bounds-check elimination (a compiler-dependent win
  our LLVM path does not need), inline varint strings, and the `own` handle design (still design-only there).

## 1. What was read, and what was not

Read: README; `docs/goose_spec.md` (sections 1.1 to 1.3, 3.5 on ADT modes); `docs/implementation.md`
(Backends, 3.4 roots and provenance, 3.5 the store rule, 3.11 recursion, 6.2 stack assignment, 7 the data
stack runtime); `docs/design/` notes `prior_art`, `relative_references`, `root_structs`, `nested_growth`,
`owning_dynamic_data`, `ownership` (summary and recommendation); `docs/stdlib.md` (`slot_pool`,
`slice_pool`); `bench/results.md`, `bench/notes.md` (layouts, caveats, what the benchmarks say about the
language), `bench/slot_pool.md`, `bench/design.md`.

Not read: the compiler source itself, serialization, coroutines, threads, the FFI, the tutorial. Not run:
Goose or any of its benchmarks. Their figures come from one Windows machine (Ryzen 9 9950X3D), compiled with
MSVC v145 and clang, whole-process wall clock, best of three after two warm-ups, and the benchmarks, their
C++ and their Rust versions were written by the language's author.

## 2. Backend

The generated C is the same for every way of building it: the system compiler for a native executable
(`-O2` default; MSVC, clang and gcc appear in the docs), or the bundled TinyCC through `goose run`. TinyCC
exists for fast edit-run cycles. Benchmarks are reported per toolchain, and the toolchain matters: for
example the `blur` kernel is 952 ms under MSVC and 266 ms under clang in their table. Their Rust rows are
single rustc timings and are compared against the clang column because rustc is LLVM.

What that means for us:

| | Goose (C backend) | Ubel PoC (LLVM IR backend) |
|---|---|---|
| Optimizer | whatever C compiler the user has | LLVM, one version, ours |
| Portability | anywhere a C compiler exists, plus a JIT | targets LLVM supports; needs clang + our C runtime |
| Control over codegen | C-level only (`restrict`, hints) | full: attributes, metadata, calling convention |
| Precise GC later | hard from C (no stack maps) | possible with LLVM statepoints |
| Startup / tooling | TinyCC JIT gives instant run | none yet |

Nothing in Goose argues for changing our backend. The one transferable observation is that their results
differ by up to 3.5x between two C compilers on the same C, so any cross-language number we publish should
name the compiler and version.

## 3. The memory model in one page

- **Data stacks.** The compiler assigns growable values to N stacks by liveness, like registers. Each stack is
  one reserved region of `GS_STACK_RESERVE` bytes (2 GB default, at most 2^48) plus an unmapped guard gap.
  Linux and macOS map the region read/write and let the kernel supply pages on demand; Windows commits pages
  from a fault handler. A scope's exit restores the stack pointer on every exit path.
- **The invariant.** Lifetimes on one stack nest strictly, and at most one growable value is live per stack,
  on top. Growth is a pointer bump with no capacity check (the reservation is the check, with the guard
  gap behind it), and references into a growing array stay valid because it never moves.
- **Fixed-size values** live in registers or the native stack; only variable-size and growable data uses the
  data stacks. The hidden `gs_sp` argument passes the current stack tops down calls.
- **Stack count is a compile-time constant** (`BoundStacks`, at most 1024 by default). That is why recursion
  is restricted: a non-fixed value that is alive in a frame across a recursive call is a compile error,
  because every activation would need a stack of its own.
- **If the OS refuses a reservation** the runtime retries at half the size down to 1 MB; the compiler's
  omitted checks assume at most `GS_STACK_RESERVE`, so a smaller region is always safe. An overrun aborts with
  a message naming the thread program, the region size and the flag that raises it.
- **Lifetime checking.** Every reference-like value carries a set of "root alternatives" (the variable whose
  scope bounds each place it may point), with a depth: 0 for globals, otherwise the scope count along the
  compile-time call path. "A outlives B" is `depth(A) <= depth(B)`. A store is accepted only if every root of
  the value outlives the destination; call arguments are never stores. Functions are specialised per
  argument-root class instead of taking lifetime parameters. No annotations.
- **Mutation-heavy code** uses pools: `slot_pool` (reusable fixed-size slots, stable addresses, LIFO free
  list) and `slice_pool` (reusable runs, first-fit with coalescing), with narrow typed links ("relative
  references") between elements. Their own notes say that independent grow and release in arbitrary order
  needs these, and that nested growth of recursive types is a research note with nothing implemented.

## 4. What the headline measures

- **Idiomatic baseline.** 3.5x is against code written the way a typical C++ or Rust author would (a heap
  node per tree node, `std::string`, `unordered_map`). The same effect is visible in our own numbers when MID
  is compared with HIGH.
- **Expert baselines.** Against hand-written arena C++ the figure is 1.24x / 1.26x, against the best safe Rust
  1.13x / 1.19x. Their `bintrees` row (large, v145 / clang): Goose local pool 206.0 / 203.8 ms; C++
  `new`/`delete` 3,208 / 3,224 ms; C++ vector arena 393 / 407 ms; Rust `Box` 3,103 ms; Rust `Vec` arena
  235 ms. So most of the 15x there is "arena versus per-node malloc"; what is left over the arenas is
  1.14x to 1.16x against the Rust arena and about 1.9x to 2.0x against the C++ vector arena.
- **The author's own caveats** (`bench/notes.md`): whole-process wall clock dilutes rows where input
  generation dominates; a freshly written executable can take 10x its steady-state time on the first
  runs, so warm-ups are required; differences under about 10% are noise (15% for cache-bound random access);
  **the malloc-bound rows, which the headline ratio rests on, are the least reproducible** (they moved up
  to 23% between back-to-back runs); reports from different
  machine states are not comparable with each other.
- **Losses are reported too.** Against the best safe Rust row Goose trails on `calc` (0.91x MSVC, 0.89x
  clang) and, under MSVC only, on `lru` (0.81x) and `scene` (0.82x); against the expert C++ rows it trails on
  `lru` under MSVC (0.88x) and `calc` under clang (0.91x). Both the MSVC and clang suite figures are
  published side by side.
- **Layout does a lot of the work.** Their table of node sizes shows e.g. a `calc` binary node of 6 bytes
  against 12 to 32 for Rust and C++, a `sexp` node with inline text and 4-byte links, and no string header
  (inline varint length). A comparison that only changes the allocator would not show this.

## 5. Side by side

| | Goose | Ubel (PoC) |
|---|---|---|
| Unit of scoped memory | per variable, on typed stacks | per `with arena` block, mixed objects |
| Arena storage | one reserved region per stack, no end check | malloc'd header + 64 KiB chunks, bump check inlined in the IR, chunk cache on destroy |
| How many arenas | fixed at compile time | one per block entered, dynamic (recursion is fine) |
| Fallback for general data | none (pools, or restructure the program) | HIGH tier: conservative mark-sweep GC |
| Lifetime safety | whole-program provenance inference, no annotations | tier tags (`ArenaRef`) + borrow rules R1 to R4 + escape rejection |
| Returning a region reference | allowed (root propagates, function specialised) | refused (R3); idea: D1 v2 |
| Layout | nested data inline, narrow links, variable-size enums | pointers to objects; every enum value is an object; strings `{len, bytes}` |
| Moving data | never (stable references) | never (D7: never move) |
| Cost of leaving the model | rewrite with pools/indices | none (use the GC tier) |

## 6. Findings that bear on our open items

### F1. D1 v2, region-polymorphic results (the most directly useful finding)

*Goose.* A function that returns a reference gets a result whose root is the argument's root; every rule then
checks that root. A value that may point to several places carries all of them and the checks use the
innermost. Temporaries are rooted one scope deeper than where they are made. Function specialisation per
root replaces lifetime parameters. ML Kit's region polymorphism and Cyclone's region handles are the
earlier forms of the same idea: the callee allocates into, or returns something rooted in, storage the
caller chose.

*Our item.* `docs/ARENA_BORROW_RULES.md` section 2 shows why naive stripping is unsound (`pick` returns its
argument; the result type carries no tag, so the borrow leaves the arena unnoticed) and section 6 says the
fix is "instantiating `L` with the caller's arena and tagging the result", blocked because
`SemaType::Named` carries no lifetime arguments.

*Suggests.* Two things from Goose make that fix more concrete. (1) Tag the result with the **innermost** arena
among the arguments whose lifetime name `L` appears in the result, which is Goose's "innermost alternative"
and is a cheap, conservative rule. That alone would turn the `pick` example from "unsound" into "accepted
inside the arena, rejected if the result is stored outside". (2) Goose keeps provenance on **values in the
checker** rather than in types, which is why it needs no lifetime arguments on named types. The same shape
would avoid the `SemaType::Named` problem for us, at the cost of carrying a provenance fact on every
reference-like value in `type_infer`.

*Cost and risk.* Sema work in files the owner edits (anchored patch only). Not prototyped. Methods and
trait methods use a separate argument loop and would need the same treatment. The pre-existing gaps
(returned closure capturing a reference parameter, a callee storing a reference into a struct passed
alongside) stay open and would be reachable through the new path unless closed at the same time.

### F2. D4, payload-free enum variants (revisit now)

*Goose.* An ADT is an integer tag (smallest integer that fits the variant count, `u8` by default) followed
by the payload. A payload-free enum is an ordinary 1-byte value. "Fixed mode" is tag plus the largest
payload, may be overwritten in place by a different variant, and allows no interior references; "variable
mode" is tag plus the actual variant's payload, allows interior references, and may never be replaced in
place. Allowing both replacement and interior references would be unsound (Grossman, "Existential Types
for Imperative Languages", cited by their spec).

*Our item.* D4 says "keep the uniform object for now, revisit once strings and closures exist". They exist.
Today every enum value is a heap object (8-byte tag then payload slots), including variants with no
payload.

*Suggests.* Two options, increasing in size. (a) For an enum in which **no variant has a payload**, use a plain
integer tag as the value (no object at all); it then works in LOW without any allocation rule, compares with
`icmp`, and `match` becomes a `switch`. (b) For mixed enums, keep objects but make each payload-free variant
a **static singleton object** (outside the heap, like string literals and non-capturing closures), so there
is no allocation and the representation stays uniform. (b) is our own idea, not Goose's; it needs a check that
nothing relies on the identity or mutability of an enum object.

*Cost and risk.* Emitter-only for (a) and (b); the sema rules do not change. The equivalence loop and the
existing enum examples are the test. Not tried.

### F3. Reserved contiguous arena regions (an experiment, not a proposal)

*Goose.* Reserving a large address range per stack removes the capacity check, makes growth a pointer bump
and keeps arrays contiguous across growth.

*Our item.* Our arena is a chain of malloc'd 64 KiB chunks with the bump check inlined. The check is one
compare and a well-predicted branch, so removing it is unlikely to matter by itself. The real benefit of a
contiguous region is that it enables narrow region-relative links (F4) and in-place growth of one large
array without copying.

*Costs for us, specific to our targets.* A 32-bit userspace (a 32-bit Termux userspace, our `-m32` runs) cannot
reserve gigabytes; Android and strict-overcommit systems may refuse large reservations (Goose retries at
half the size, down to 1 MB, for the same reason). A reservation or `mmap` per `with arena` block would be
slower than our chunk cache for the common small, short-lived arena; Goose avoids that because it
reserves once at program start for a fixed number of stacks, which our dynamic per-block arenas cannot
do. A hybrid (a region cache for large arenas on 64-bit, chunks everywhere else) is possible but is more
machinery than the check it removes.

*Suggests.* Measure before deciding (see E3). Do not change the design on Goose's numbers.

### F4. Narrow typed links ("relative references")

*Goose.* A link stored as a 1, 2 or 4-byte offset, either from the field itself (self-relative) or from a
named pool's base. Both make a structure position-independent and 1 to 4 bytes per link instead of 8. The
self-relative form costs about 1.5x on `lru`, which relinks six links per hit, and nothing measurable on
build-once-then-walk code; the pool-relative form closed most of that gap.

*Our item.* Our links are full pointers (8 bytes, 4 on 32-bit). Narrow links need a contiguous region, so
this depends on F3. It would also change the GC's view of pointers in scannable arenas.

*Suggests.* File it as a possible later layout optimisation, relevant to D6 only as an alternative to pointers,
not as something D6 needs.

### F5. D6, recursive arena structures

*Goose.* Recursive and mutated structures live in pools, linked by optional references (`T?`) or narrow links;
the design notes call growing a recursive tree in place a research note, with nothing implemented. A
recursive-cycle function may only store references into globals, pool parameters or locals of an enclosing
non-cycle function.

*Our item.* D6 asks how arena structures may refer to themselves (optional references, arena-capable enums,
or arena lists).

*Suggests.* Two observations. First, the emitter side is easy for us (a link is just a pointer into the arena);
the blocker for D6 is the same as for F1, provenance that types cannot express. Solving F1 by per-value
provenance would likely move D6 forward too. Second, Goose confirms this is a hard problem for a bump
region: they avoid it with pools rather than solving it.

### F6. Stable-address pools and LOW slices

*Goose.* `slot_pool`: fixed-size slots, stable addresses (the backing array never moves), `alloc` takes the
most recently freed index else appends, `free` checks `0 <= i < len` and records the index; there is **no
occupancy or generation tracking**, so a double free or a use of a freed slot is an "application error" and a
reference to a freed slot can observe the slot's later replacement. `slice_pool`: runs of elements,
first-fit in index order with coalescing, backed by an augmented B+ tree of 512-byte nodes. A generational
handle pool (`gen_pool`) and linear `own` handles are discussed in `docs/design/ownership.md` but are not in
the standard library; the note's own recommendation is to decide the vocabulary and build nothing yet.

*Our item.* The "stable-address mechanism" and "LOW slices" items in the suggested-next list.

*Suggests.* (1) A stable-address slot pool is small once addresses are stable; we get stable addresses from
the non-moving GC (D7) and from malloc'd chunks, so no mover is needed. (2) Be explicit about the semantics we
would promise: Goose's pool is **memory-safe but not logically safe** (reuse is visible through old
references). If LOW is ever to promise no use-after-free of a slot, it needs generations, and Goose has not
shipped that. (3) `slice_pool`'s first-fit B+ tree is a general-purpose allocator; it is more than a first
LOW-slice version needs.

### F7. Behaviour at limits (OOM policy per tier)

*Goose.* Overrun aborts with a message naming the thread program, the region size and the flag that raises
the size, written without stdio because it comes from a signal handler. The reservation falls back by
halving to 1 MB, and the compiler's checks stay valid because a smaller region only fails earlier.

*Suggests.* Our runtime aborts today with no policy. Cheap improvements, independent of everything else:
make the abort message name the tier, the size requested and the setting that raises the limit (for the
GC heap, the 256 MB default). Do not copy the halving fallback: it relies on the reservation model.

### F8. Benchmark methodology for A10 (adopt)

From `bench/notes.md`, `bench/slot_pool.md` and `bench/design.md`:
- interleaved samples, two discarded warm-ups, process-cycle counts rather than wall time, identical
  checksums across implementations, compiler and library fingerprints and **all raw samples** saved;
- for an A/B of a compiler change, builds from the same sources minutes apart, with `-falign-loops=32` under
  clang so a shifted hot loop is not mistaken for a codegen change;
- a layout table (`sizeof`) per benchmark so allocator effects and layout effects can be told apart;
- compare against **both** an idiomatic baseline and an expert arena baseline, and report memory (peak
  working set) next to time;
- treat differences under about 10% as noise, and expect allocator-heavy rows to be the least reproducible.

*Our item.* Our "MID is 2.7x to 11x faster than HIGH" is an arena against our own collector, the same shape
as Goose's "vs idiomatic" ratio, and has the same sensitivity. A10 should add a malloc/free baseline and a
hand-written C arena baseline for each workload, and a layout table, before any cross-tier ratio is quoted
as evidence.

### F9. Prior art we can cite (per Goose's survey; we have not read the papers)

- **ML Kit region inference** (Tofte et al.): stack-like regions, region-polymorphic functions; the
  retrospective reports fragmentation from "infinite" regions and that a garbage collector was later added to
  reclaim what region inference leaks. Supports an arena tier with a GC fallback.
- **Cyclone**: LIFO arenas are "useful when callers know object lifetimes but only callees can determine
  object sizes" and "not suited" to servers and event loops, which is why it added unique pointers,
  reference counting and keyed regions. Matches our MID/HIGH split.
- **Real-time Java scoped memory**: an object in an outer area may not reference one in an inner area, checked
  at run time; the same store rule as ours, checked statically in sema.
- **Second-class parameters** (Osvald et al. 2016, Hylo, Chapel, D `scope`): references that may be passed
  down but not stored or returned are the family our R1 to R4 belong to.
- **GNU obstacks**: the closest runtime-library form of "one growable object at the top".

## 7. What not to copy, and why

- **The recursion restriction.** A dynamic-size local cannot be held across a recursive call. Our arenas are
  per block and heap-headed, so recursion with arenas is unrestricted; keep that.
- **No GC tier.** Goose gets its speed by making the whole language fit stacks and pools. We keep HIGH for
  data that does not fit, which is a feature, not a gap.
- **Bounds-check elimination.** Their A/B shows large gains under MSVC (+81% to +191% on the three `blur`
  rows) and roughly none under clang (-6% to +7% across the suite, which they class as noise): LLVM already
  removes most of what is provable. Not a priority.
- **Inline varint strings and a full inline layout.** Large language-level changes; only their effect on
  size is worth remembering.
- **`own` handles.** Design-only there, with an explicit "build nothing yet".
- **Threads, serialization, the JIT.** Out of scope for the PoC.

## 8. Where our design differs on purpose, so we keep it

Dynamic arena count (recursion and nesting are free); a GC tier for arbitrary graphs; per-function tier
choice; arena values holding pointers to GC data through scannable arenas; no program-wide specialisation
(Goose's per-root specialisation costs compile time and code size, which we do not pay).

## 9. Suggested experiments (none started)

- **E1 (small).** Payload-free variants as static singletons or plain integers (F2); count allocations in the
  enum examples before and after; the equivalence loop is the test.
- **E2 (medium).** D1 v2 prototype in a scratch clone (F1): result tagged with the innermost arena of the
  arguments tied to `L`; the `pick` case must be rejected when the result is stored outside the arena and
  accepted inside it. Sema is in files the owner edits, so it ends as an anchored patch.
- **E3 (small, isolated C).** Reserved-region arena against our chunked arena (F3): allocation throughput for
  16 to 256 byte objects, and the cost of entering and leaving a block for small (1 KB) and large (64 MB)
  arenas, on 64-bit. Also check whether the reservation succeeds under Termux and on Android generally.
- **E4 (medium).** A10 baselines (F8): malloc/free and hand-written C arena versions of the existing
  workloads, a layout table, and the sampling protocol above.
- **E5 (optional).** Build Goose in the sandbox and run `bintrees` next to a C arena on the same machine as
  our runs, to see the arena-versus-malloc gap without the Windows and MSVC setup. Not needed to act on
  F1, F2 or F7.

## 10. Sources in the Goose repo

`README.md`; `docs/implementation.md` (Backends; 3.4 roots and provenance; 3.5 the store rule; 3.11
recursion; 6.2 stack assignment; 7 data stacks); `docs/goose_spec.md` (1.1 size classes, 1.3, 3.5 ADT modes,
3.9 relative references); `docs/design/prior_art.md`, `relative_references.md`, `root_structs.md`,
`nested_growth.md`, `owning_dynamic_data.md`, `ownership.md`; `docs/stdlib.md` (`slot_pool`, `slice_pool`);
`bench/results.md`, `bench/notes.md`, `bench/slot_pool.md`, `bench/design.md`.
