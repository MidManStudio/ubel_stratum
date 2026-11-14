# The LLVM backend: where it stands, and the road to a real executable

Status: **provisional**. Written 2026-11-09 from master `044017b` plus the arena delivery of the same day. What is
*measured* below was measured in that session (the commands are given); *reasoned* means it follows from what was
measured but was not tried; *decision* means the language owner has to choose. Sizes are relative, in "deliveries"
of the kind this backend has had so far (one emitter slice with examples, tests and documentation), not in days:
the history is the only calibration there is, and it is thin.

This document sits on top of `docs/LLVM_ROADMAP.md` (the design decisions D1 to D8 and the tracks) and
`docs/LLVM_POC.md` (what was built and measured). Its job is the question those two answer badly now: *where does
the backend stand, and how far is it from compiling real Ubel programs to executables?* The "Where things stand"
table in `LLVM_ROADMAP.md` stops at slice 4 and should be replaced by a pointer here (a patch is offered at the end).

## 0. The short answer

- **A native executable can already be made, for a subset, by hand.** `ubel-llvm emit prog.ubl -o prog.ll`, then
  `clang prog.ll runtime/ubel_rt.c runtime/ubel_main.c -o prog`. That is what `scripts/run_equiv.sh` does for 33
  programs and the target program at -O0, -O2 and under the sanitizers, at 64-bit and 32-bit. `ubel build` and
  `ubel-llvm build` exist since M1: one command from source to a native executable, with a clang 15 or newer on the PATH.
  The `ubel` command line has `check`, `run`, `build` and `debug-tokens`, and the LLVM crate is a member of the main
  workspace.
- **The subset is small next to the language.** Measured on current master: the target program
  (`examples/arena_demo/ast_builder.ubl`) compiles; the self-hosted tokenizer does not; **4 of the 168 `ok_`
  fixtures compile** (they are sema snippets, so this measures which features the language tests use, not how many
  programs would compile, but the direction is not in doubt). The interpreter runs the language; the backend
  runs a corner of it.
- **The corner is the part that was hardest to design, not the part that is largest.** The three memory tiers
  (a conservative GC for HIGH, arenas for MID with borrow rules, no-allocation LOW), strings with full Unicode,
  closures, enums, `match`, and a runtime with a tested collector exist and are checked against the interpreter. What
  is missing is mostly breadth: collections, numeric types beyond `int`, generics, traits, modules, a standard
  library, and the command that ties it together.
- **Three bars, three distances** (reasoned, section 5): a one-command executable of a toy program is close (one or
  two deliveries); the self-hosted tokenizer is a medium road (collections, floats, `Str`, `assert`); programs like the
  ones in `tests/fixtures` need generics, traits and the rest of the type system, which is a long road and partly
  waits on the traits work.
- **The name was out of date, and is fixed.** The folder is now `crates/ubel_llvm` (section 7).

## 1. What exists

Measured with `wc -l` and the equivalence suite on master plus the arena delivery.

| Part | Size | Notes |
|---|---|---|
| Emitter, `crates/ubel_llvm/src/emit.rs` | 3,077 lines | Ubel AST to textual LLVM IR. No libLLVM dependency; clang assembles the IR. |
| C runtime, `runtime/ubel_rt.c`, `ubel_rt.h`, `ubel_main.c` | 1,374 lines | Conservative mark-sweep GC with large objects, arenas (also scannable ones), strings, Unicode string methods, global roots, a stress mode. |
| Generated Unicode tables, `runtime/ubel_unicode.h` | 546 lines | From Rust's standard library, per toolchain (`scripts/gen_unicode.rs`). |
| Equivalence suite, `scripts/run_equiv.sh` | 72 entries | Every program equals the interpreter at -O0, -O2, ASan+UBSan, 64-bit and `-m32`; refusal tests; the harness. |
| Collector and runtime tests, `harness/rt_gc_test.c` | 17 tests | Run in four build modes. Mutation-tested by hand throughout. |

Supported today, with the example that shows it (all in `crates/ubel_llvm/examples/`):

| Area | Works | Examples |
|---|---|---|
| Scalars and control flow | `int`, `bool`, functions, `if` as a statement and an expression, `while`, `loop`, `for` over an integer range, `break`/`continue`, recursion, scopes, interpolation in `println` | 01 to 07, `fail_div_zero` |
| Memory tiers | HIGH (the GC), MID (`with arena`, cleanup on every exit, borrowed `&arena` parameters, rules R1 to R4, scannable arenas), LOW (no allocation) | 08, 09, 18, 25 |
| Structs | plain structs in every tier, edge structs with lifetime parameters | 10 to 14 |
| Enums and `match` | payload variants, payload-free variants as static objects (E1), `match` on enums and on strings | 15 to 17, 28, 31 |
| Strings | values, `+`, comparisons, interpolation, `len`, `is_empty`, `contains`, `starts_with`, `ends_with`, `to_upper`, `to_lower`, `trim`, `trim_start`, `trim_end`, `replace` with Rust's Unicode semantics; a result built from an arena string lives in that arena | 19, 20, 24, 27 to 30, 32 |
| Closures | capturing and non-capturing, in every tier under the D2 rule | 21 to 23, 26 |
| Target program | `ast_builder.ubl` equals the interpreter | `examples/arena_demo` |

## 2. What the language has that the backend does not

The emitter's own list of refusals is the authority (87 distinct messages in `emit.rs`). Grouped:

| Group | Refused today |
|---|---|
| **Types** | `double`/`float` (no float literals or types), `short`/`ushort` casts, `char`, tuples, generic types and structs and enums (so `Pool<T>` and `Handle<T>` too), `&mut`, references to anything but a struct, qualified type names, fallible types |
| **Collections** | `List<T>`, `Dictionary`, `Queue`, `InlineList`, `Unique<T>`, `Shared<T>`, indexing, `push`, `len` on a list, `for` over anything but an integer range |
| **Declarations** | traits, `impl` and `extend` blocks, struct methods and properties, attributes (`@derive`), `static` items and other declaration kinds, generic functions, `async`/`await`; imports do not exist because the language has no modules yet |
| **Calls** | your own functions, `println`/`print`, `assert` and `panic` (a string message only); no other builtins (`len`, `push`, math, ...), no named arguments, no calling a value that is not a local |
| **Patterns and control** | guards (`where`), nested patterns, struct-style variants, destructuring `let` and `for`, `break` with a value, an `if` expression that lacks an `else` |
| **Operators** | most operators on bools, any operator on structs, enums or closures |
| **Strings** | format specifiers in interpolation, `chars`, `split` (they return lists) |
| **Memory** | allocation in LOW (decision open), building a string or a struct in MID outside `with arena` (decision open), `with gc`/`heap`/`pool`, `Pool<T>`/`Handle<T>` |
| **Closures** | captures of mutable variables, closures that capture more than 30 variables, a closure inside a closure inside an arena |

How much does each group matter? The first refusal of each `ok_` fixture, measured with the binary after M2a
(`tests/fixtures/ok_*.ubl`, 176 files; **28 of them compile now**, they were 4 of 168 before M2a; a program refused for one
reason is usually refused for several, so these count what a program would hit first, and the ranking is more reliable than
the numbers). The 148 that are still refused:

| First refusal | Fixtures | What would unlock it |
|---|---|---|
| a value or annotation of an unsupported type (`List<T>` 12, `InlineList`, `Dictionary`, `Unique` 3 each, `Queue`, `Shared`, `&mut`, `Callback`, `char`, `double`, and 12 annotations of other types) | 38 | collections (M3), `float`/`double` (M2b) |
| trait, `impl` and `extend` declarations | 28 | traits (the traits chat) |
| generic structs, types and functions | 26 | monomorphization (decision) |
| attributes on structs (`@derive`) | 21 | derives, which wait on traits |
| references that are not to a struct, struct-style variants, `static`, constants, struct methods, other blocks, format specifiers | 35 | several small slices |

The self-hosted tokenizer (137 lines, the project's own next target in `self-hosted/docs/SELF_HOSTED.md`) is refused
at its `double` field (M2b; its `Str` type is accepted since M2a, example 37), and would then need `List<int>`, `List<Token>`, `List.new()`, `push`, indexing and
`len`. That is a short, concrete list.

## 3. From source to an executable, today and as it should be

Today: `ubel build prog.ubl` (M1b, 2026-11-13) and `ubel-llvm build prog.ubl -o prog` (M1a, 2026-11-10) do the whole
route in one command: IR, the embedded C runtime, one clang invocation. `ubel build` checks the file first with the
CLI's own diagnostics. `ubel-llvm emit` still writes just the `.ll`, and the scripts still run clang themselves.

What a user would expect: `ubel build prog.ubl -o prog`. What that needs, beyond the language coverage:

1. **A driver.** *Done in M1a* (`crates/ubel_llvm/src/driver.rs`; `LLVM_POC.md`, "Building an executable"). Emit to a temporary file, find a C compiler, link, report errors. The emitter has no libLLVM
   dependency, which makes this small. Whether to keep "clang assembles the IR" or link LLVM directly (for example
   through `inkwell`) to drop the clang dependency is a decision (*decision*): linking LLVM is a large dependency
   and a build-time cost, and nothing yet argues for it.
2. **The runtime shipped with the compiler.** *Done in M1a for the C runtime and the committed Unicode header, which are
   embedded in the crate and compiled on every build; there is no cache and no regeneration of the header.* The C runtime and the generated Unicode header would be embedded in
   the `ubel` binary (or installed next to it) and compiled by the driver. The Unicode header is tied to the Rust
   toolchain that builds the interpreter (see `LLVM_POC.md`, "String methods that build a string"); a release build
   would regenerate it once, as `run_equiv.sh` already does when the versions differ.
3. **A toolchain contract.** *Partly done in M1a: clang 15 or newer is checked with `--version` (Apple's clang is not held
   to the number), the flags are the ones `run_equiv.sh` uses.* Which clang versions, which flags. Everything has been tested with clang 18 on Linux
   x86-64 and i386 only. Windows, macOS, ARM64, Termux and the CI runners are **not tested** for anything since
   slice 3.
4. **Runtime failure behaviour.** The interpreter and the native code agree on the programs in the suite, which
   include one failing program (`fail_div_zero`). User-facing panics with source
   locations, integer overflow policy for the fixed-width types, bounds checks on indexing and an out-of-memory policy
   (today the runtime aborts) are not designed.
5. **Layout in the repository.** *Decided and done in M1b (2026-11-13): `ubel_llvm` joined the main workspace* (a member,
   not a default member), the CLI depends on it, and `ubel build` calls its library; the shell-out alternative was
   not needed. The workspace lockfile is version 4, which the 1.75 cargo of the earlier work cannot read, so M1b was
   verified with Rust 1.85. The committed `Cargo.lock` is from the initial commit and lists only `ubel_stratum`; cargo
   rewrites it on any build, so it was left alone. The cost of joining: the workspace release profile (LTO, one
   codegen unit) applies to the emitter; the suite scripts switch that off for their own builds.

## 4. The roadmap

Each milestone has an exit that anyone can check. Sizes: S is one delivery, M two or three, L four to six, XL more
than that or dependent on language work that is not done. These are estimates from this backend's own history
(slices 1 to 4 and A5 to A9 took a few chats each), not commitments.

| # | Milestone | Why | Depends on | Size | Exit |
|---|---|---|---|---|---|
| M0 | Subset and target program | done | | done | `ast_builder` equals the interpreter in the suite |
| M1 | `ubel build` and packaging (done: M1a `ubel-llvm build`, M1b `ubel build` and the workspace join) | the thing a user can try | nothing in the language | S to M | `ubel build examples/x.ubl -o x && ./x` on Linux, and CI does it through the real CLI |
| M2 | Numeric tower and `assert` (M2a done: sized integers, `assert`, `panic`; M2b open: `float` and `double`) | floats and sized integers are in the tokenizer and everywhere else | overflow behaviour is decided for the sized integers (wrap, as the interpreter does); plain `int` in the interpreter saturates through `f64`, a core bug (`LLVM_POC.md`, known differences) | M | the fixtures that first fail on `u8`, `u64`, `double`, `assert` compile and match the interpreter |
| M3 | Collections v1: `List<T>`, indexing, `push`, `len`, `for` over a list, bounds checks | most of the 42 | the generics decision (below) for `List<T>` | L | the self-hosted tokenizer compiles and equals the interpreter on its inputs |
| M4 | Generics (monomorphization) and traits (static dispatch), `@derive` | 24 + 25 + 21 first refusals | the traits chat; decision on monomorphization versus uniform representation | XL | the generic and trait fixtures compile; `Dictionary`, `Unique`, `Shared` follow |
| M5 | Modules, imports and a minimal standard library (arguments, stdin and files, exit code) | real programs, the self-hosted parser | language work: modules do not exist yet | L, language-dependent | a multi-file program builds; the self-hosted parser compiles |
| M6 | LOW for real: allocation, `Unique`, moves, `Pool`/`Handle`, `dyn` traits | the third tier is today "no allocation" | D-decisions on LOW (B3), the `dyn` work now in `core` | XL | a LOW-tier program that owns and frees memory, checked by the sanitizers |
| M7 | Shippable: Windows, macOS, ARM64, debug info, panics with locations, OOM policy, precise or threaded GC if needed, a release process | users on other machines | M1 | XL | the `ubel` release builds and runs an example on three operating systems in CI |
| M8 | Self-compilation | the stated self-hosting goal | M3, M5 (and more) | XL | the self-hosted tokenizer and parser compile themselves |

Order: M1 first (it is cheap and it makes everything after it demonstrable), then M2 and the `assert` slice (cheap,
they unblock the most fixtures per line), then M3 with the generics decision made explicitly before it starts. M4 and
M6 are large and depend on other chats' work; they should be scoped with those chats, not inside the emitter's.

**The generics decision.** `List<T>` forces it: either monomorphize (a copy of the code per element type, like Rust
and the interpreter's type checker already assume) or use a uniform representation (every element is a pointer-sized
slot, like our enum payloads and closure captures). Uniform is a smaller first step and fits the collector; it makes
`List<int>` boxed or needs a second representation. *Decision*, with the self-hosted tokenizer's `List<int>` and
`List<Token>` as the first test of whichever is chosen.

## 5. How far, in three bars (reasoned)

1. **A toy program from one command.** M1 only. Anything in the 33 programs already builds; the missing piece is the
   command and the packaging. *One or two deliveries.*
2. **The self-hosted tokenizer.** M1, M2 (floats, `Str`, `assert`) and M3 (lists). The tokenizer is 137 lines and uses
   exactly those. *Perhaps ten deliveries, with collections and the generics decision as the bulk.* This is the
   first point at which "a program the project actually wrote runs natively" is true.
3. **The programs in the fixtures, that is, what the interpreter runs today.** M1 to M6. The fixtures use generics,
   traits, collections and wrappers in the first lines. This is a long road, and the order in which the language
   itself settles (traits, modules) sets part of its pace.

The honest summary: the backend is not far from *an* executable and a long way from *the language's* executables, and
the gap is breadth, with two decisions (generics, LOW) in the way of the largest pieces.

## 6. What is settled and what is open

Settled in practice (each has code and tests): textual IR assembled by clang; a C runtime; a conservative collector
that never moves objects (D7: never move); arenas as chunk lists with an inlined bump; strings as `{length, bytes}`
UTF-8 objects with literals outside the heap; closures as `{code, captures}` objects; payload-free enum variants as static
objects; the result of a string-building method in the receiver's arena when there is one.

Open (see `LLVM_ROADMAP.md` for D1 to D8; the new ones are marked):
D4 (the plain-integer form for unit-only enums, the roadmap row still says "keep the uniform object"), D6 (recursive
structures in an arena; sema rejects an arena enum value in a struct field today), D1 v2 (region-polymorphic results,
the idea in `GOOSE_STUDY.md` F1), LOW's allocation story (B3), OOM policy, thread safety, alignment contract, the
**generics decision** (new, section 4), the **`ubel build` and workspace layout** (new, section 3), and where the
runtime tables for new Unicode versions come from in a release.

## 7. The name, and the folder

`crates/llvm_poc` said "proof of concept" long after the concept was proven (the equivalence suite, the measured tier
costs, the target program). Done on 2026-11-10, as its own delivery: the folder is **`crates/ubel_llvm`**, the owner's
choice. The Cargo package (`ubel_llvm`) and the binary (`ubel-llvm`) already had that name, so the build did not change.
`docs/LLVM_POC.md` and `docs/LLVM_ROADMAP.md` keep their names for now as the history; renaming documents is cheap and can
follow.

How it was done: one scaffold template, with `rename_files::` for the directory and `patch_files::` for every
`crates/llvm_poc` path in 24 files (comments, usage lines, documents, the replacements manifest). Left alone on purpose:
the scratch directory name `ubel_llvm_poc` under `TMPDIR` (a throwaway path in `run_bench.sh` and `llvm-proc.yml`) and the
workflow file names `llvm-poc-emit.yml` and `llvm-proc.yml`.

Two cautions that stay true after the rename:

- The two workflows were changed in a separate step, by hand (reasoned, not tried: a run that pushes with the default
  token cannot change files under `.github/workflows`).
- Any tarball made before the rename carries the old paths, and the replacements applier matches members by path, so one
  applied after the rename would recreate `crates/llvm_poc` next to the new folder.

## 8. Next five actions

1. Done on 2026-11-10: the folder is `crates/ubel_llvm` (section 7).
2. Done: the pointer in `LLVM_ROADMAP.md` and the Phase 4 row of `web/site/src/project-status.md`.
3. Done on 2026-11-13: M1b, `ubel build` in the CLI and the workspace join (section 3, point 5).
4. M2b: `float` and `double`, including printing them exactly like the interpreter does (Rust's shortest round-trip `{}`), tested
   differentially against Rust itself. M2a (sized integers, `assert`, `panic`) is done.
5. Make the generics decision (section 4) before starting collections, with the self-hosted tokenizer as the test.

## 9. How this was measured, and what was not

- Coverage: `ubel-llvm emit` over `tests/fixtures/ok_*.ubl` (168), `self-hosted/tokenizer/*.ubl` (1),
  `examples/arena_demo/ast_builder.ubl` and the 33 non-`u_` examples, with the binary built from master plus the arena
  delivery; first refusal per file, grouped by message. Counts are first refusals, not full feature censuses.
- Sizes: `wc -l`. References to the old folder name: `grep -rIl llvm_poc`.
- Not measured: how long any milestone takes; CI on real runners for anything since slice 3; Windows, macOS, ARM64,
  Termux; `ubel build` does not exist, so nothing here tests it; the fixtures were written to test sema, so "4 of 168"
  is a lower bound on what a real corpus would say only in the sense that a real corpus does not exist yet.
