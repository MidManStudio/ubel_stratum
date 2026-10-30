# Ubel Stratum: roadmap for the LLVM proof of concept

Status: **provisional**. Written 2026-10-25 as the plan that sits on top of `docs/LLVM_POC.md`
(what has been built and measured) and `docs/GC_RESEARCH.md` (checked notes on collectors).
Every statement tagged *probed* was checked by running the front end on a small program in the
session that wrote this; *reasoned* means it follows from what was measured but was not tested;
*decision* means the language owner has to choose.

The plan has two tracks. **Track A** is engineering that is mostly decided: grow the emitter,
one small slice at a time, until it compiles `examples/arena_demo/ast_builder.ubl` and the output
matches the interpreter. **Track B** is a design phase: five pieces that need decisions before
more machinery is built on top of them, because the emitter slices that remain all allocate more
kinds of object and every one of them leans on the answers.

## Where things stand

| Slice | What it adds | Evidence |
|---|---|---|
| 1 | scalars, control flow, tier calls, `with arena` with cleanup on every exit | CI, both architectures |
| 2 | plain structs, GC allocation in HIGH, arena allocation inside `with arena` | CI, both architectures |
| 3 | edge structs, `&x`, `&L T` fields, lifetime parameters | applied to master, CI run not reported yet |
| 4 | enums with tuple payloads, `match` (statement and expression) | local only: 36 of 36 at 64-bit and 32-bit |

The target program's first refusal is now its `string` payload (`Var(string)`), so the next
blocker is strings, and behind strings closures.

---

## Track A: emitter slices to the target program

Each slice ships with examples that must match the interpreter at -O0, -O2 and under ASan+UBSan,
a refusal test for what is still out, and a deliberately broken emitter that proves the new test
can fail.

| # | Slice | Needs first | Notes |
|---|---|---|---|
| A5 | GC large objects | nothing | **Done in the runtime, provisional** (`docs/GC_LARGE_OBJECTS.md`): size classes to 32 KiB, large objects as runs of spans, pointer-free objects of any size (`ubel_gc_alloc_noscan`), pointer-holding arrays of any size (`ubel_gc_alloc_array`). Tested in C at -O0, -O2 and under ASan+UBSan, 64- and 32-bit, with mutation checks. The emitter does not use it until A6. |
| A6 | Strings as values | A5, decision D3 | `Var(string)`, `show`, `apply`, `==` on strings, printing a string value, interpolation that builds one. Where interpolation allocates follows the same site rules as a struct literal. |
| A7 | Closures | decision D5 | Non-capturing first (code pointer plus a uniform closure object), then capturing ones. Completes `build_and_eval` and `compile_expression`. Sema already accepts closure calls from every tier (see B4), so the tier of a function type has to be settled before this is emitted. |
| A8 | Target program compiles | A5, A6, A7 | The emitter output must equal `ast_builder.expected`; the program moves from the guard in `run_equiv.sh` into the equivalence suite. |
| A9 | Beyond the target | A8 | Collections in each tier, generics (monomorphization), traits (the traits chat), sized integers and floats, `for` over collections, `Unique`/`Pool`/`Handle`. Each is its own design question; none is needed for the target. |
| A10 | Evidence | A8 | Cross-language baselines (C, Rust, Go, C#) with matching object sizes and warm-up, a small real program in Ubel, a Mid Engine style workload, and the benchmark methodology items (warm-up, dispersion, raw samples, hardware counters). |

Parallel to A5 to A8, and not blocking them: run the slice 3 and 4 CI workflows on both
architectures, and the GC hardening list in B1.

---

## Track B: the design phase

Suggested order: **B4 first**, because it is the specification the others must satisfy, with
**B5 started alongside it**, because the matrix is the statement of the invariants a proof
needs. Then B2 and B3, then B1, whose design depends on what the other four allow.

### B4. The cross-tier communication matrix

This is a first draft, filled from what the front end does today (*probed*), with gaps marked.

**Who may call whom** (every pair was checked; "yes" means sema accepts it).

| caller \ callee | HIGH | MID | LOW |
|---|---|---|---|
| HIGH | yes | yes | yes |
| MID | **no** | yes | yes |
| LOW | **no** | **no** | yes |

A tier may call itself and the tiers below it. The rule is clean and is the base of everything
else.

**What may cross a call** (*probed* unless marked).

| Value | Down a call (HIGH to MID or LOW, MID to LOW) | Into a closure called from MID | Back up (returned) |
|---|---|---|---|
| int, bool | yes | yes | yes |
| GC struct or enum (same pointer, aliased) | yes (HIGH to MID and HIGH to LOW) | yes | yes, when it came in as a parameter |
| arena value (struct, enum, `&L` reference) | **rejected by sema**, including to a `&L` parameter of a named function | **rejected** | **rejected** (`&arena T` cannot outlive its block) |
| closure | HIGH to MID yes; capturing an int or a GC value yes | n/a | not probed |
| struct built in LOW | accepted by sema when returned to HIGH | n/a | n/a |

Three things in that table need a decision.

1. **No arena value can cross any call today.** The Stage 1A benchmark's callback pattern
   (MID builds a tree in an arena and hands a pointer into it to a HIGH closure) and the
   ast_builder sketch's `parse_expr_tree` both assume it can. In the language as written the only
   things that cross are ints, strings and GC values, so arena data is built and read inside one
   function. Either the language grows a borrowed-arena-reference parameter that sema accepts
   (with its lifetime rules), or the callback pattern is dropped in favour of copy-out, which
   measured 1.2x to 1.3x slower than building in the GC heap directly.
2. **Closures are an unchecked channel across tiers.** A `@tier(low)` function can call a
   function-typed parameter, and sema accepts a closure that allocates GC structs being passed
   into it (*probed*, including the allocating case). That breaks the premise that makes LOW cheap
   and safe in the PoC: "LOW code allocates nothing and calls nothing in another tier, so no
   collection can start while it runs" (Stage 1A measured a LOW boundary of 0 to 0.4 ns on that
   basis). The call lattice above checks named calls only. A function type needs a tier, and
   calling a closure from tier T needs closure tier at most T.
3. **Where a MID helper outside `with arena` allocates** is undefined (open decision 3 in
   `MEMORY_MODEL.md`). Because arena values cannot cross calls, the answer may fall out of
   decision 1: a MID helper could simply not be able to build values for its caller.

*Deliverable:* a table like the two above that is complete (every value kind against every
boundary, including closures, returns, captures, stores into fields and collections), each cell
marked allowed, forbidden or copied, with the reason, and one test in `tests/fixtures` per cell.

### B2. Arenas and allocation types

What exists: `with arena(size) { }` with size treated as a chunk size, plain and scannable
arenas in the runtime, edge structs with lifetimes, and per-site allocation decided by where a
literal is written (inside `with arena`: the innermost arena; in HIGH: the GC heap). The grammar
also has `with gc`, `with heap` and `with pool` blocks, which the emitter refuses and nothing here
specifies.

What the probes found: a plain struct cannot hold an arena struct (reported as escaping, even from
the same arena), an `edge struct` with `&L` fields can; enums cannot take a lifetime and cannot hold
arena references; `&L T?` accepts neither `null` nor any "none" spelling; so a **recursive arena
tree cannot be written today**, and the ast_builder rewrite uses fixed-shape node graphs. That is
a language gap, not an emitter one.

To settle:

1. Whether an arena's size is a chunk size (today) or a hard limit, and what happens on exhaustion.
2. How an arena that holds pointers into the GC heap is declared: the Stage 1A hazard (a plain
   arena holding the only reference to a GC object loses it to the collector) is reproduced on CI.
   Options measured so far: scannable arenas (18% to 28% faster than a GC-owned list on that
   workload), arena kind chosen by the compiler from the stored types, root registration, handles,
   or forbidding the store.
3. How a recursive arena structure is expressed: optional arena references, arena-capable enums,
   or arena-resident lists.
4. The allocation-site rule as a table: tier, lexical context and type against allocator, with
   `with gc`, `with heap` and `with pool` either specified or removed from the grammar.

*Deliverable:* the allocation-site table, the arena size and exhaustion rules, one chosen answer
for the hidden-root hazard, and the language change (or refusal) for recursive arena structures.

### B3. The LOW tier

What exists: LOW functions are accepted by sema and can read GC data passed in (arena values
cannot be passed to them, see B4); LOW struct literals are accepted by sema but refused by the
emitter; `Unique.new(...)` passes sema but the
interpreter panics on a field read through it, so LOW construction has no reference oracle;
move checking exists in sema. Measured (provisional): a LOW call boundary costs 0 to 0.4 ns; LOW malloc/free ran from
about parity to 1.7x slower than the GC heap when little data stays live, and 1.4x to 2.4x faster
than it with a 6.4 MB live set.

To settle:

1. How LOW allocates and frees (`Unique`, malloc/free, stack values, or an explicit allocator),
   and whether a LOW struct is a value or a reference (the interpreter treats every struct as a
   reference).
2. What a LOW function may hold while a collection could run. This depends on B4 item 2: it is
   only safe to hand LOW raw pointers into the GC heap if nothing it can call allocates there.
3. Whether GC objects can be pinned for LOW, and whether the collector may ever move objects
   (which would forbid raw pointers outright).
4. The interpreter has to run `Unique` so there is something to compare LOW code against.

*Deliverable:* LOW's allocation and ownership rules, the pinning rule, and `Unique` working in
the interpreter, so slice 5 of the emitter (LOW) has an oracle.

### B1. A proper garbage collector

What exists, and is measured (two shared CI runners, hand-written IR, provisional): a
stop-the-world, non-moving mark-sweep collector with size-class spans, a conservative stack scan
and precise heap masks per object. Marking costs 10 to 15 ns per live node, a 6.4 MB live set
spent 52% to 59% of the run in collection with 2.2 to 3.1 ms pauses, and allocation costs 7 to
14 ns against 1.5 to 5 ns for an arena bump. Limits: objects of at most 64 words, globals are not
roots, one thread.

Hardening that does not need a design decision, and gates everything after it:

1. Global roots and a collect-on-every-allocation debug mode run over the whole example suite
   (large objects, A5, are in the runtime).
2. Stress tests that put objects in every place a pointer can live (struct and enum payloads,
   closures when they exist, scannable arenas) and check survival and reclamation.

To settle, in this order:

1. **Roots.** The stack scan is conservative, which is an assumption about the C compiler and
   stack layout, not something the language can promise, and it cannot work on wasm (no stack
   to scan). A precise alternative is a shadow stack or compiler-generated stack maps.
2. **Moving or not.** Compaction and generations need a moving collector, which conflicts with raw
   pointers handed to LOW (B3).
3. **Generations.** An arena already behaves like a nursery that dies wholesale; whether HIGH needs
   its own is a measurement question (the notes in `GC_RESEARCH.md` say what barriers cost).
4. **Concurrency and pauses.** Only after the above, and only if a workload needs it.

*Deliverable:* a collector design note with the choices above made and justified by measurements
from the example suite, and the hardening list green on both architectures.

### B5. The proof

What can be proved, and what cannot, stated plainly. A proof is about a **model**, not about the C
runtime or about LLVM. The practical shape is a small core calculus of Ubel (functions, structs,
enums, closures, three tiers, arena blocks, a GC heap, owned LOW values) with a small-step
semantics, and theorems about it:

1. **Type safety:** progress and preservation for the tiered calculus.
2. **Region safety:** no dereference of an arena object after its block has ended (the outlives
   relation is the invariant).
3. **Collector safety:** every object reachable from the roots is retained, with the roots
   defined by the model (this is where the hidden-root hazard shows up as a missing premise:
   a plain arena must either be scanned or forbidden to hold the only reference).
4. **Boundary lemmas:** one per cell of the matrix in B4. "No GC allocation while LOW runs" is
   one of them, and the closure finding above is a counterexample to it in today's sema.
5. **Ownership safety for LOW:** no use after free, no double free (linear moves).
6. **Compilation correctness** for the emitted subset: the IR simulates the model. Differential
   testing against the interpreter, which is what the equivalence suite does, is evidence for
   this, not proof.

Approach and prior art: this is the shape of the region-calculus line of work (Tofte and Talpin's
region inference, Cyclone) and of the Rust-semantics line (RustBelt); those are the references to
read first for structure. Mechanizing it in a proof assistant (Lean, Coq or Isabelle: a choice for
the owner) is optional and can come after the paper proofs.

Steps: (a) write the invariants as the B4 matrix; (b) write the semantics for the fragment
arena + outlives and prove 2; (c) add the GC heap and prove 3; (d) add closures and tiers and prove
type safety; (e) check each theorem against the implementation: sema must reject exactly what the
calculus rejects, so any difference is a bug in sema or in the model, which is the most useful output
of the exercise; (f) property-test the C runtime against a reference model of the collector.

What the PoC contributes: executable evidence, a place to find counterexamples (two so far: the
hidden-root hazard, and closures bypassing the tier lattice), and the matrix as the statement to
prove. What it cannot give: proof of the conservative scan, which rests on compiler and stack
behaviour, or of the C runtime without a separate verification effort.

*Deliverable:* the core calculus written down, the proofs for 2 and 3 on paper, and a list of every
place sema and the calculus disagree.

---

## Decisions needed from the language owner

| # | Decision | Blocks | Default I would take if told to proceed (provisional) |
|---|---|---|---|
| D1 | Can an arena value cross a call (a borrowed-reference parameter), or is copy-out the only way? | B4, B2, A7 | Allow a borrowed `&L` parameter with a lifetime tied to the caller's arena, because the callback pattern is the design's main selling point and copy-out costs 1.2x to 1.3x. |
| D2 | Does a function type carry a tier, and is calling a closure checked against it? | B4, B3, A7 | Yes, to close the hole above. |
| D3 | String representation: GC object `{length, bytes}`, literals static, interpolation allocates by site. DECIDED as D3b (chat 4): sema tracks arena strings, a run-time `+` always allocates in the GC heap, see docs/ARENA_STRINGS.md | A6 | Sema half delivered; emitter half is A6. |
| D4 | Enum payload-free variants: uniform heap object (today) or plain integer | A4 | Keep the uniform object for now, revisit once strings and closures exist. |
| D5 | Capturing closures: closure object holds captured values, in the GC heap or the arena by site | A7 | By site, like every other literal. |
| D6 | Recursive arena structures: optional references, arena-capable enums, or arena lists | B2, A8 | Needs your call; the target program does not depend on it. |
| D7 | Moving collector: allowed or never | B1, B3 | Never, until a measurement shows non-moving costs too much. |
| D8 | Proof assistant, or paper proofs only | B5 | Paper first. |
