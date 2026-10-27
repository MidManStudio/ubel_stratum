# Memory references: what the collected papers say, and what Ubel can take from them

Status: **provisional**. Written 2026-10-27 (LLVM PoC, chat 3). A digest of seven PDFs and three
pasted AI texts about memory management, filtered for Ubel Stratum's three tiers (HIGH = GC,
MID = arena, LOW = manual ownership). It sits next to `docs/GC_RESEARCH.md` (checked notes on
collectors) and `docs/LLVM_ROADMAP.md` (the plan). It changes no decision: D1 stays decided, and
every "lead" below is an idea to weigh, not a rule.

Tags, in the same spirit as `GC_RESEARCH.md`:

- **checked**: confirmed in this pass against a primary source, by reading the paper's own listing
  or table, or by redoing its arithmetic.
- **wrong** / **imprecise**: contradicted by, or looser than, what the sources support.
- **not re-checked**: plausible, not verified here. A lead, not a fact.
- **reasoned**: follows from what was read, but was not tested. Nothing here was executed; every
  code reading (the papers' listings and `crates/llvm_poc/runtime/ubel_rt.c` at master a4ee32e) is
  static.
- **lead**: something Ubel could adopt. Not a decision.

---

## 1. What was read

| id | source | how much was read | how far to trust it |
|---|---|---|---|
| S1 | Devkota, Chitrakar. "Dynamic Memory Allocation: Implementation and Misuse", Medicon Engineering Themes 6.4 (2024) | all | Good description of glibc's heap and a useful catalogue of heap bug classes. A student review article; the exploit sections overstate a little (section 5). |
| S2 | Shpak, Kurchenko, Shcheblanin. "Reducing the vulnerability level of garbage collection using arena allocator", Cybersecurity: Education, Science, Technique 4 (32), 2026 | all, including the three code listings | Weak as evidence (section 5). Useful as a statement of the arena pattern and as a code sample to review. |
| S3 | Firouzi, Sami, Khomh, Uddin. "On the use of C# Unsafe Code Context: An Empirical Study of Stack Overflow", ESEM 2020 | all | Useful for what `unsafe` is used for. The method and a few counts have problems (section 5). 2018 data. |
| S4 | Ferrara, Logozzo, Fahndrich. "Safer Unsafe Code for .NET", OOPSLA 2008 (the file is named `oopsla07.pdf`, the paper says OOPSLA'08) | all; the formal sections skimmed, not re-derived | Strong. Peer reviewed, and its tables are internally consistent (checked). |
| S5 | Zulfiqar. "High-Performance Concurrent Memory Allocation", Waterloo MMath thesis, 2022 (the llheap allocator) | chapters 1 to 4 and the conclusion; the chapter 5 graphs were not read | Solid engineering write-up from one group; the performance results are theirs. |
| S6 | Jansson. "JEFF and STEVE: A Relocatable Memory Allocator and its Benchmarking Friend", Chalmers MSc 2015 | chapters 1 to 5, 6.1, 6.3 to 6.5 and 9; the implementation walk-through (6.2) and the results chapter (8) were not read | Interesting ideas, weak numbers (32-bit, single-threaded, few and small applications). |
| S7 | Dubach (after Hendren, Krolik). "Compiler Design, Lecture 22: Garbage Collection", McGill slides, winter 2026 | all (text only, figures not viewed) | Textbook level. Reliable for the algorithms and their costs. |
| P1 | Pasted AI text: reading values safely from data serialized in an arena (PDF parsing) | all | Mostly sound, one imprecise rule (section 5). |
| P2 | Pasted AI text: GC code touching manually allocated memory | all; its links were not opened, except the Snowflake paper | Direction right, two statements muddled (section 5). |
| P3 | Pasted AI text: C# unsafe code | all | Accurate where it could be checked. |

Not extracted on purpose: S1's step-by-step exploit walk-throughs (kept at the level of what each
shows about defences), P1's PDF-specific parts (cross-reference tables), and the per-allocator
graphs in S5 and S6.

---

## 2. The short version

1. **Arena checklist (B2).** Alignment, an O(1) bump, an oversize path, no dangling pointer after
   free, an explicit exhaustion rule. S2's own example code fails the first four (section 3.1).
   The PoC runtime handles the bump cost and oversize requests; alignment of later allocations
   has not been checked.
2. **D1 is about extent and neighbours as much as lifetime (B4).** S4's `GetBytes` example shows
   what goes wrong when a callee is handed a caller-region pointer: a wrong length, or an
   override scribbling on the caller's other data. Lifetimes (`&L`) cover the first half of D1.
   Lead: borrowed arena references are typed, with no pointer arithmetic, and anything array-like
   carries its length in the value (section 3.4).
3. **D2 is the same hazard as S4's virtual methods.** A caller-supplied function is unknown to the
   checker. S4's fix was contracts inherited by overrides; Ubel's equivalent is a tier on function
   types, checked when a closure is passed (reasoned, section 3.4).
4. **Three ways to keep addresses stable (D7, B3.3):** never move (Go, the PoC today), scoped
   pins (C# `fixed`), or handles with lock/unlock (S6). The default "never move" agrees with every
   source. S6 is the only one that moves memory under an API, and its evidence is weak.
5. **Pause cost follows live objects, not garbage.** S7's cost model and the PoC's 10 to 15 ns per
   live node agree, and the one real-world case that was checked (Discord's Read States service)
   says the same (section 3.2).
6. **A GC does not make the runtime immune to use-after-free.** CVE-2025-49844 (checked) is a
   use-after-free in a garbage-collected scripting engine. The hidden-root hazard is the same
   family. Lead: collect-on-every-allocation over the whole suite, already in B1.
7. **S1's bug classes, mapped to LOW:** use-after-free and double free are for linear moves to
   prevent by construction (S1 shows detection-based defences get bypassed); invalid free needs
   values that cannot be forged from integers; overflow needs extent; disclosure is outside what
   tiers can do (section 3.3).
8. **Keep GC metadata out of the objects.** The PoC does (checked in `ubel_rt.c`: pointer bitmap,
   allocation and mark bits are side tables). S1 says in-band metadata is what heap corruption
   bugs abuse, so keep this property when large objects arrive (A5).
9. **C# is moving `unsafe` from "a block that grants permission" to "an obligation on callers plus
   a marked, audited use site"** (checked against Microsoft's diagnostics list and dotnet/runtime
   pull requests). Lead for B3's escape hatch.
10. **Evidence (A10):** the interpreter can write allocation traces itself, giving real lifetime
    data without Valgrind (S6's idea), and S5's micro-benchmark shapes give the PoC's churn test a
    comparable form.

---

## 3. By Ubel question

### 3.1 MID: arenas (roadmap B2)

**What the sources agree on.** An arena groups objects by lifetime and frees them with one call
(S2 part 4; S6 section 2.3; S7's example of freeing 100 integers one by one, which could have been
reclaimed as a block). Locality: an allocator controls spatial locality, and an allocation buffer
that carves objects lazily from a block is the same idea as an arena without the free step (S5
sections 2.2.2 and 2.7). Keeping chunks for reuse instead of returning them to the OS is common
practice (S2; S5's heap reuse stack, which reuses the hottest storage first).

**S2's example arena against the PoC runtime** (both read, not run):

| concern | S2, code block 3 | PoC `ubel_rt.c` |
|---|---|---|
| cost of a bump | walks the chunk list from the head on every allocation, so cost grows with the number of chunks (reasoned from the listing; contradicts the paper's O(1) claim) | the newest chunk is first in the list, so the bump is O(1) |
| alignment | no padding between allocations | chunk header is 16 bytes so the payload starts 16-aligned; later allocations are aligned only if the emitter rounds sizes. **Not checked in `emit.rs`.** |
| request larger than a chunk | guarded by an `assert` against the node size, not the usable payload. With asserts off, or for sizes within one header of the node size, the retry loop never ends and keeps allocating nodes (reasoned) | `ubel_arena_grow` installs a dedicated chunk sized to the request |
| after free | the "free nodes" mode leaves the arena's first-chunk pointer dangling, so reuse reads freed memory (reasoned) | `ubel_arena_destroy` frees the arena itself, nothing to reuse |
| reuse without returning to the OS | three reset modes (reset sizes, zero memory, free nodes) | up to 64 default-size (64 KiB) chunks are cached across arenas; no "rewind and keep" mode |
| bookkeeping | `totally_allocated` is reset but never incremented (reasoned) | n/a |
| out of memory | `calloc` result used unchecked in `arena_make_node` (reasoned) | prints a message and aborts |

Leads: a rewind mode only if a workload shows repeated fill-and-clear cycles; the alignment
contract needs one explicit sentence and one test with a struct whose fields force an odd
size (see question N2 below).

**Open question B2.1 (chunk size or hard limit).** Nothing read argues for a hard limit. S2 and S5
both treat the block as a chunk. A hard limit only made sense in S6, which targeted a device with
512 KB of RAM.

**What transfers from P1.** Two rules: reads carry their extent (a slice of pointer plus length,
and a read fails when `n > remaining`, never `current + n <= end`, which can overflow), and data
handed to another layer must not outlive the arena. `&L` lifetimes already enforce the second
(`MEMORY_MODEL.md` section 6). "Use offsets, not pointers" matters only for arenas that move (one
contiguous growable block) or are written to disk. Chunk-list arenas never move (S2, PoC), so it
does not apply to `with arena` as designed.

### 3.2 HIGH: the collector (roadmap B1)

- **Cost model (S7).** Mark-sweep costs O(N+E) because the sweep visits every object; copying costs
  O(R+E) in the live objects only, but wastes half the heap and must find and update every
  pointer; reference counting is incremental and cheap to reason about, but cannot collect cycles.
  The PoC is non-moving mark-sweep at 10 to 15 ns per live node (`LLVM_POC.md`), consistent with the
  first model (reasoned). S7 also states the assumption every one of these needs: sizes and
  pointer fields are known per record. The PoC supplies that through its pointer masks.
- **A checked real-world case (Discord).** The service they moved from Go to Rust was Read States,
  which keeps an LRU cache of tens of millions of objects. Latency and CPU spikes came about every
  two minutes, because Go forces a collection at least that often even without heap growth, and
  the spike size tracked the cache size (they shrank and partitioned the cache to shrink the
  spikes). Two things for Ubel: pause cost follows the live set, and a time-based trigger is a
  design choice (the PoC paces by allocated bytes with a floor). S2 retells this as request
  volume and packet loss; the blog does not say that (section 5).
- **A GC'd runtime can still have use-after-free.** CVE-2025-49844 (CVSS 10.0): an authenticated
  user running a crafted Lua script can manipulate the collector, trigger a use-after-free and
  possibly reach remote code execution; it affects all Redis versions with Lua scripting and is
  fixed in 8.2.2 (all checked). S2 explains the mechanism as a string object never anchored in the
  root set, then swept while the script still held it. That has the same shape as the hidden-root
  hazard (an object reachable only from memory the collector does not scan), but the explanation
  is S2's, not the advisory's. Lead: collect on every allocation across the example suite (already
  B1 hardening item 1), and add a case where a fresh object is reachable only from a plain arena or
  from a local not yet stored anywhere.
- **Moving.** S7's copying and compaction need every pointer located, which conflicts with the
  conservative stack scan (as `GC_RESEARCH.md` already says). See 3.3 for the third option, handles.
- **Metadata placement.** The PoC keeps the pointer bitmap (one bit per pointer-sized heap word)
  and the allocation and mark bits outside the objects (checked in `ubel_rt.c`). S1 shows what
  in-band metadata costs: headers and free-list links inside heap memory are exactly what
  corruption bugs overwrite. When A5 adds large objects, prefer a side table or a span kind over a
  header inside the object. If intrusive free lists ever appear, S1 describes glibc's hardening
  (the stored link is XORed with its own address shifted right by 12 bits); not needed while free
  slots are bitmaps.
- **Weak references (S7).** Weak maps let a compiler hang side data on IR nodes and have it vanish
  with the node. In Ubel, side tables for AST nodes can live in the same arena as the nodes, so
  nothing is needed; weak references only matter for HIGH-tier caches. Low priority, nothing to do now.
- **Later, concurrency.** S5 measured that a lock or atomic on the allocation fast path costs a lot,
  and that per-thread heaps with no fast-path synchronization came closest to the best allocators;
  freeing another thread's object needs a separate "away" list (S5 section 3.3). Go's per-P caches
  are the same idea (`GC_RESEARCH.md`). Only relevant once the runtime stops being single-threaded.

### 3.3 LOW: ownership and the unsafe escape hatch (roadmap B3)

**S1's bug classes against what Ubel has.**

| class | what it needs from LOW | note |
|---|---|---|
| use after free | an owner that dies or moves exactly once | move checking exists in sema (`MEMORY_MODEL.md` section 9) |
| double free | the same | S1 shows one detection (the fast-bin "head of the list" check) defeated by freeing a second block in between, and the other (the tcache key) side-stepped by using the fast bins once the tcache is full. Prevent by construction; do not rely on a runtime check |
| invalid free | free accepts only an owned value, and no integer-to-pointer conversion | `Unique` does not run in the interpreter yet (B3 item 4) |
| heap overflow | the extent of any buffer travels with the pointer (S4: a region is a pointer plus its writable bytes) | struct fields have static extent; arrays and buffers do not |
| disclosure | outside what tiers do | S1 ties it to defeating address randomization |

**Three ways to keep addresses stable.**

| mechanism | where | cost and limits |
|---|---|---|
| never move | Go, the PoC today | needs the heap to be non-moving; pins are free |
| scoped pin | C# `fixed`; S4's concrete semantics carries a set of pinned objects for exactly the block | only matters if D7 flips to "moving allowed" |
| handles with lock/unlock | S6: a block can be moved only while unlocked; compaction runs when the app is idle | an extra indirection and a manual lock discipline; ranked first on speed and below dlmalloc on memory in its own chapter 9 summary (against dlmalloc, jemalloc and tcmalloc, on replayed traces); 32-bit, single-threaded, no alignment. Weak evidence |

**What C# unsafe is used for (S3, 2,283 Stack Overflow snippets, 2018 dump).** Image or pixel access
27%, interoperability 21%, raw pointers 20%, memory management 15%, `stackalloc` 9%, performance
8% (figure 3). Reading for LOW: it needs a bounds-carrying slice for buffer work, a foreign-call
story (the Mid Engine packet codec is the obvious customer) and stack buffers. C# has been moving
the common cases out of `unsafe` (S3 notes `stackalloc` outside unsafe contexts since C# 7.2; as
far as I know that is only when the result is a span, not re-checked). That is the direction to
copy: make the common LOW operations safe and keep the raw escape rare.

**A model for the escape hatch (checked, with a caveat).** Microsoft's diagnostics list and
dotnet/runtime pull requests describe "updated memory safety rules": `unsafe` on a member declares
an obligation on its callers and no longer opens an unsafe context in its own body; using such a
member needs an unsafe block or expression at the use site (CS9360 to CS9363), an unsafe override
of a safe member is an error (CS9364), and migration fixers insert audit comments. P3's mention of
those codes is right. Caveat: it is behind an opt-in mode in what I read, and I did not check which
release makes it default. Lead: if LOW gets an escape hatch, make it an obligation on callers plus a
use-site marker.

**Safe wrapper around an unsafe core (S4 section 2.2, P3 "isolation").** Called "pervasive" in
`mscorlib`. Ubel's analogue is a LOW core behind a HIGH or MID wrapper, which the call lattice
already allows.

### 3.4 Boundaries: D1 and D2 (roadmap B4)

**D1.** S4 section 2.4: a method gets pointers into its caller's stack buffers plus a length. The
two dangers it names are a wrong length and an override that overwrites the caller's neighbouring
data through the same pointer. Neither is a lifetime problem; `&L` handles the lifetime half.
Leads for the D1 spec, none of them decisions:

- a borrowed arena reference is a typed reference with no pointer arithmetic, so a callee cannot
  reach neighbours;
- anything array-like carries its length in the value;
- "callee may read, not store or return" is the lifetime rule already decided.

S4's contract style is a model for the checking: a precondition on the callee (writable bytes at
least n times element size) is assumed at entry and asserted at every call site. That maps onto a
`[lifetime L]` parameter.

**D2.** The same example is the closure hazard. An unknown subclass can replace the method, so the
checker cannot see the callee. S4's answer was contracts inherited by overrides, and it caught the
off-by-one in the subclass because of it. Reasoned analogue: a function type carries a tier, and
every closure passed there inherits and must satisfy it.

Rows the B4 matrix does not have yet: "pointer plus extent", "borrowed arena reference as a
parameter", "closure capturing an arena reference".

### 3.5 Proof (roadmap B5)

S4 is a small worked example of the style B5 wants: an idealized instruction set (a handful of constructs),
a concrete semantics with an error state for an out-of-region access, an abstract semantics derived
step by step, and soundness theorems, so an analysis that never reaches the error state proves the
property. B5's steps (b) and (c) can follow the same shape: error states for "dereference after
the region ended" and "dereference of a collected object", with sema as the abstraction (reasoned).

It also gives a cost envelope for static extent checks (checked by redoing the arithmetic from its
tables): Stripes combined with intervals and linear equalities validated 57.96% of unsafe accesses
(weighted by access count; the unweighted mean of the table is 56.22%, and the introduction's
"more than 54%" is stale) at about 12.9 ms per method over 113,034 methods. Polyhedra were 34 and
78 times slower on the two largest assemblies, with a 2-minute timeout per method, and validated
fewer accesses, probably because of the timeouts (reasoned). If LOW ever needs static extent checks,
expect cheap domains to settle about half automatically and the rest to need contracts or runtime
checks. The paper says the false alarms are mostly missing contracts.

Limits: intraprocedural, 2008 .NET 2.0 assemblies, no contracts used, one property only.

### 3.6 Large objects and strings (roadmap A5, A6)

- Practice elsewhere: glibc sends requests above about 128 KiB to `mmap`, flags those chunks, and
  hands them straight back to the OS on free (S1); llheap has buckets up to 4 MB and maps above a
  threshold (S5 section 3.3); .NET's large object heap is not compacted by default
  (`GC_RESEARCH.md`, checked). Consensus: large objects live in a separate space, outside size
  classes, and do not move.
- Ubel today: the collector aborts above 512 bytes (`ubel_gc_alloc`, read). Strings are the first
  customer, and a pointer-free large object needs no scanning, only marking. That is A5 step 1.
  Lead: keep large-object membership in a side table or a span kind (section 3.2).
- S5's header remembers the request size next to the bucket size. Size-class rounding is internal
  fragmentation, and the PoC's stats count class sizes (`alloc_bytes += elem`, read), so a
  requested-bytes counter would show what rounding costs. Small change.
- S5 section 3.3.2: in the C API, `realloc` silently dropped zero-fill and alignment. Ubel's HIGH
  zeroes and MID and LOW do not (`LLVM_POC.md`, limits), so a grown MID buffer can expose garbage
  that a HIGH one would have zeroed. If growable strings or arrays arrive (A6, A9), decide whether
  growth keeps these properties.

### 3.7 Evidence and testing (roadmap A10 and B1 hardening)

- **S5's benchmark shapes.** Churn: K threads, M slots; free the object in a random slot and allocate
  a new one; knobs for threads, slots, objects per thread, min, max and step size, and size
  distribution. Speed chains such as malloc-free and malloc-realloc-free with a warm-up. Cache thrash
  and scratch (false sharing, needs threads). A memory-footprint benchmark for blowup and
  fragmentation. Prior art it builds on: threadtest, shbench, Larson. Leads: give the PoC's churn the
  same shape and knob names so results compare, and add a footprint benchmark (peak against
  requested); the PoC measures time only.
- **S6's trace and replay.** Record every new, free and access of an unmodified program, derive
  each block's lifetime (operations between its new and its free) and replay against allocators. It
  needed a patched Valgrind and about 30 minutes for one web page load. Ubel has an advantage: the
  interpreter is already the oracle and could write allocation events (site, size, tier, birth,
  death) itself. Lead: use that to answer `GC_RESEARCH.md` open question 6 (young-object mortality)
  on real programs, starting with `ast_builder.ubl`. Caveat: "death" needs a definition per tier
  (arena end for MID; HIGH needs a liveness rule, S6 used last access).
- **S6's testing habits.** Fill each allocation with a byte derived from its address, run tens of
  thousands of random allocations and frees, and verify nothing else changed; and plant off-by-one
  bugs by hand to check the tests fail. The second is the mutation habit already used on this
  project. Lead: add the address-derived fill check to the runtime self-test for GC objects and
  arena chunk reuse.
- **S5's statistics and debug mode.** Per-heap counters with no atomics, a debug build with pre and
  post conditions, an unfreed-storage report at exit, and startup and shutdown hooks so allocations
  outside the program proper do not count. Their cost: about 30% with both on (their number).
  The PoC already has `UBEL_STATS` and an arena-live counter (`ubel_arena_live`, whose header comment says a correct program ends at 0).

---

## 4. Questions the sources raise that the roadmap does not have

These are suggestions for the language owner. They are not in `LLVM_ROADMAP.md` and are not
decisions.

| # | question | from | note |
|---|---|---|---|
| N1 | Out-of-memory policy for each tier | S5 3.7.1 | The runtime aborts with a message everywhere today (read), so the policy exists but is not written down. S5 argues recovery is almost never practical. |
| N2 | Alignment contract: every allocation 16-aligned, or aligned to the type? | S5 3.3.1, S2 | Check `emit.rs` rounding; add one test. |
| N3 | Stable-address mechanism if D7 ever flips: scoped pin or handles | S4, S6 | No action while D7 is "never". |
| N4 | LOW slices: bounds carried and checked at run time, or proven statically | S4, P1 | The two are not exclusive: a checked slice plus static elision. |
| N5 | Stats additions: requested bytes, large-object counters, bytes live at exit | S5 | Small. |
| N6 | Allocation-event trace written by the interpreter | S6 | Feeds A10 and the generational question. |
| N7 | Do grown buffers keep zero-fill and alignment? | S5 3.3.2 | Only when growable strings or arrays exist. |

---

## 5. Fact-check log

**Pasted texts**

| claim | verdict | note |
|---|---|---|
| P1: "ensure `current + N <= end`" | **imprecise** | The addition can overflow. The safe test is `n <= end - current`. The pasted C snippet itself uses the safe form (`size < bytes_to_read`), so prose and code disagree. |
| P1: use offsets, not pointers, in arena data | **right only in some designs** | True for one contiguous growable block or serialized data. Chunk-list arenas never move (S2, PoC). |
| P1: copy with `memcpy` instead of casting a misaligned pointer | **not re-checked** | Standard C practice. |
| P2: a GC drops the wrapper before the manual block is released, causing use-after-free or a leak | **imprecise** | Dropping the wrapper without freeing is a leak. Use-after-free is the other orders: the manual block freed while a reference survives, or the wrapper collected while native code still uses its buffer. |
| P2: raw addresses are treated as opaque scalars | **depends on the collector** | Precise collectors with type maps: yes. A conservative scan treats any word that points into the GC heap as a pointer and ignores the rest (`LLVM_POC.md`: arena pointers are outside the GC heap and ignored). |
| P2: research designs combine .NET with manual heaps under ownership rules | **checked** | Project Snowflake (Parkinson et al., OOPSLA 2017): a manual heap beside the GC heap in .NET, objects with a single owning pointer, deallocation at any program point, concurrent sharing. The "stack- or heap-bound owners" wording was not checked. Its links to Utah, Wisconsin, ResearchGate and Scribd were not opened. |
| P3: `AllowUnsafeBlocks`, `fixed` pins movable objects | **checked** | Microsoft's unsafe diagnostics page. `delegate*` function pointers: **not re-checked**. |
| P3: compiler diagnostics CS9360 to CS9363 | **checked** | They exist and belong to the updated memory safety rules (section 3.3). |
| P3: "Safer unsafe code for .NET" is a formal safety analysis | **imprecise** | It is a sound static bounds analysis for a model of unsafe code (S4), not a safety proof for the language. |
| P3: Microsoft's best-practices page says to keep unsafe blocks small, wrap them, and use `Debug.Assert` | **not re-checked** | The page was not opened. The practice matches S4's wrapper pattern. |

**S2 (arena against GC)**

| claim | verdict | note |
|---|---|---|
| the C standard defines `malloc` as at least ten thousand lines | **wrong** | The standard defines an interface, not an implementation. |
| CVE-2025-49844 is a GC use-after-free in Redis's Lua, rated 10.0 | **checked** | Real, CVSS 10.0, fixed in 8.2.2, needs an authenticated user running a script. |
| it went unnoticed "for a short period" | **wrong** | Reported as about 13 years old (a secondary report of the discoverer's write-up). |
| the mechanism: a string never added to the root table | **not re-checked** | The advisory only says the collector is manipulated. |
| 10.0 because Redis holds "SQL objects" and trial and error finds something | **wrong** | Redis is not SQL. The CVE record rates it network-reachable, low complexity, high impact. |
| Discord left Go because of request volume and packet loss | **wrong** | Blog: the Read States service, a huge LRU cache, spikes about every two minutes, Go's forced collection every two minutes. |
| the benchmark shows GC time grows O(n^2), "exponentially" | **wrong as stated** | Quadratic is not exponential. Also: GC runs stop at 30,000 objects while arena and malloc go to 10,000,000, and the script records `%e` (elapsed seconds) while the axis says ms. It shows that one small collector scales badly; the PoC's paced collector marks at 10 to 15 ns per live node. |
| arenas "can leak with no consequences" | **imprecise** | Memory stays held until the arena is freed; for a long-lived arena that is unbounded growth. |
| arena allocation is O(1) | **wrong for its own code** | Tail walk (section 3.1). |
| C++ "has GC from C++11 to C++23" | **not re-checked** | As far as I know those were optional hooks, not a collector, and were removed in C++23. |
| Minecraft allocates up to 1 GB/s | **not re-checked** | Cited to a video. |

**S3 (C# unsafe study)**

| claim | verdict | note |
|---|---|---|
| counts and percentages | **checked** | 599 + 1,684 = 2,283; 73.76%, 27.4%, 20.6%, 8.9% and 35.0% all reproduce. |
| `stackalloc` is the "third category with 9%" | **inconsistent** | Its own figure 3 puts raw pointers (20%) and memory management (15%) above it. |
| 12 uses of `rand()` among 67 dangerous calls | **imprecise** | In listing 2 `rand(1, n)` is an array function from the ILNumerics library, not C's `rand`. The `strcpy` and `memcpy` in listings 3 and 4 are in the posts' C++ code, not in C# unsafe blocks. The regex counted names anywhere in the post. |
| 73.76% of unsafe posts lack the tag | **indicative** | The 1,684 were found by keyword heuristics plus manual checks, so the figure depends on that method. |

**S1 (glibc heap)**

| claim | verdict | note |
|---|---|---|
| bin and chunk structure, bit flags, tcache layout | **consistent with my knowledge, not re-checked** | One typo: the listing names the forward link `phd`. |
| Safe-Linking formula `(pos >> 12) ^ ptr` | **not re-checked** | Matches my recollection of glibc. The paper gives no glibc versions; as far as I recall, Safe-Linking arrived in 2.32 and the randomized tcache key in 2.34. |
| a fake chunk on the stack bypasses "W^X, stack canaries" | **overstated** | It sidesteps canaries (no linear overflow). W^X is a separate property (reasoned). |

**S4 (Clousot)**: no errors found. Average 57.96% reproduces as the access-weighted figure; 12 ms per
method reproduces as 12.9; "orders of magnitude" for Polyhedra is loose (34 to 78 times).

---

## 6. Coverage gaps

Not read: S5 chapter 5 (graphs of llheap against glibc, dlmalloc, hoard, jemalloc, ptmalloc3,
rpmalloc and tbbmalloc); S6 chapter 6.2 (Jeff's implementation) and chapter 8 (results); S7's
figures. Not opened: the P2 links other than the Snowflake paper's publication pages; Microsoft's
unsafe best-practices page; the dotnet/runtime pull requests beyond their descriptions. Not done:
the arena and collector checks in section 3 are static readings of master, nothing was run, and the
pending `ubel-enum` archive's changes to the runtime and emitter were not considered.

## 7. Links

- Redis advisory: https://redis.io/blog/security-advisory-cve-2025-49844/
- Discord, "Why Discord is switching from Go to Rust": https://discord.com/blog/why-discord-is-switching-from-go-to-rust
- Project Snowflake (MSR-TR-2017-32): https://www.microsoft.com/en-us/research/publication/project-snowflake-non-blocking-safe-manual-memory-management-net/
- C# unsafe diagnostics (CS9360 and neighbours): https://learn.microsoft.com/dotnet/csharp/language-reference/compiler-messages/unsafe-code-errors
- dotnet/runtime migration fixers: https://github.com/dotnet/runtime/pull/131581 and https://github.com/dotnet/runtime/pull/131337
