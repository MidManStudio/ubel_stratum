# GC large objects and the array allocator (roadmap A5)

Status: **provisional**. Written 2026-10-30 (LLVM PoC, chat 3). The runtime side of A5 is done;
the emitter does not use it yet, because Ubel source has no strings or growable collections until
A6. Everything here describes `crates/llvm_poc/runtime/ubel_rt.c`, tested by
`crates/llvm_poc/harness/rt_gc_test.c`.

## What changed

- **Size classes up to 32 KiB.** The original ten classes (16 to 512 bytes) are unchanged; 17 more
  follow (640, 768, 896, 1024, 1280, 1536, 1792, 2048, 2560, 3072, 4096, 5120, 6144, 8192, 10240,
  16384, 32768), so a 600-byte string no longer needs a whole span.
- **Large objects above 32 KiB.** Each takes a run of whole 32 KiB spans inside the same reserved
  heap. The first span is `SPAN_LARGE` and holds the size, the mark bit and a no-scan flag; the
  rest are `SPAN_CONT` and point back at it. The run comes from the lowest stretch of free spans
  that is long enough, else from fresh heap. The existing range check in `mark_ptr` and the
  one-bit-per-word pointer bitmap cover large objects without any new structure.
- **Pointer-free objects of any size.** `ubel_gc_alloc_noscan(size)`, or `ubel_gc_alloc(size, 0)`.
  A large one is marked and never scanned.
- **Pointer-holding objects of any size.** `ubel_gc_alloc_array(elem_size, count, elem_ptrmask)`:
  `count` elements, each with the pointer layout of one 64-bit mask. `ubel_gc_alloc(size, mask)`
  with a non-zero mask and `size > UBEL_GC_MAX_SMALL` aborts and names the array call.
- **Long objects are scanned from the bitmap** (`scan_words`) when they have more than 64 words.
  Objects of 64 words or fewer take the original single-read path.
- Stats gain `large_allocs` and `large_bytes`; the stats line prints them last.

## Choices, all provisional

| choice | why | cost |
|---|---|---|
| a large object is rounded up to whole spans | no new heap structure | up to 32 KiB wasted per object (50% for a 32 KiB + 1 byte object, 25% at 64 KiB + 1) |
| interior pointers keep an object alive; a pointer one past the end does not | matches small objects (a past-the-end pointer would name the neighbour) | a program that keeps only an end pointer can lose the object |
| a run is found by a first-fit scan of the span table | simple, lowest address first, bounded by the span count (8192 for the default 256 MB heap) | O(spans) per large allocation when enough free spans exist; free runs fragment |
| freed runs go back to the free-span list, not to the OS | keeps reuse cheap | resident memory stays after a large object dies |
| a reused run is zeroed with `memset`; fresh heap is already zero | the allocation contract is zeroed memory | the memset is most of a large allocation's cost |
| out of heap aborts with the size asked for | same as before | none new |

## Tests

`rt_gc_test` has 10 tests, each in its own process: every class boundary and its neighbours,
large objects (sizes, accounting, drop), interior and one-past-the-end pointers, reuse and zeroing
of a freed run, arrays of pointers (large array, array of large children, 64-word elements, a
size-class array of more than 64 words, a small array whose slot has slack), pointer-free objects
that reuse memory with stale pointer bits, a random alloc/free stress with frequent collections,
and the refused misuse cases. Objects are filled with a pattern derived from their id and checked
before they are dropped. `run_equiv.sh` builds and runs it at -O0, -O2 and under ASan+UBSan, on
64-bit and (with `EXTRA_FLAGS=-m32`) 32-bit, so no workflow change is needed, and the CI report
has a row for it.

Roots are a table in a scannable arena, not locals. Live sizes come from the runtime's own
statistics, not from a copy of its tables. In the sanitizer builds (`-DRT_SOFT_LIVE`) the live
check is one-sided (`got >= expected`): instrumented frames have redzones and spill slots that no
portable code can zero, and a stale word can keep one more object alive, which a conservative
collector is allowed to do. Freeing something still referenced fails in every build.

Mutation testing: breaking each mechanism in a scratch copy turns a test red for interior pointers
into later spans, the sweep of large objects, zeroing of a reused run, the array's pointer bits,
the one-past-the-end bound, the size lookup, the free-run search, the stale free-stack skip,
scanning of large objects, scanning of long size-class objects, the sweep of continuation spans,
and the clearing of stale pointer bits. Shrinking one class (640 to 576) is an equivalent mutant:
the lookup table is derived from the class list.

## Performance (one shared VM, noise about 1 ns, so read these as "no change", not as results)

- Small objects, original runtime against this one: 10.3 to 10.4 ns per allocation against 9.1
  to 11.5 ns; churn with a live set 30 to 35 ns against 29 to 31 ns. A first version of this
  change was about 1 ns slower, because it routed small objects through the general path; the
  small path is now the original code with everything bigger moved out of line.
- Large pointer-free allocation, zeroed on reuse: about 1.5 us at 40 KB, 4.4 us at 100 KB, 48 us at
  1 MiB, all about 22 GB/s, so the zeroing dominates.
- A collection with 40 MiB of large pointer-free objects and 20000 small nodes live: mark about
  260 us, sweep about 1 us. Large pointer-free objects add almost nothing to a collection.

## Found while testing

- A scannable arena is scanned in full, including bytes nobody wrote. Chunks come from `malloc`,
  so a retired chunk can hold stale pointers from recycled memory and keep garbage alive (seen
  under ASan, where recycled mark-stack buffers were the source). Zeroing a scannable chunk on
  install, or recording how much of a retired chunk was used, would remove it. Not changed here.
- Zeroing the stack below a test function has to happen inline in that function: the bytes just
  below a frame become the next callee's frame header, which a callee cannot clear. Even then,
  sanitizer redzones keep a stale word alive, hence the one-sided check above.

## What the emitter needs next

A6 (strings) allocates a string with `ubel_gc_alloc_noscan(8 + length)`, or with the size class
the length falls in, and needs no mask. A list or array of structs uses `ubel_gc_alloc_array` with
the element's mask, which the emitter already derives for structs. A struct with more than 32
fields, and an enum variant with more than 31 payload fields, are still refused by the emitter;
they were never limited by the collector.
