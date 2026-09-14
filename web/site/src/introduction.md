# Ubel Stratum

*The right memory model for every function.*

Ubel Stratum is a statically-typed systems and game-development language.
Its defining feature is a tier system: every function declares which memory
strategy it uses, and the compiler enforces the rules statically.

| Tier | Annotation | Memory | Async | Use case |
|------|-----------|--------|-------|----------|
| HIGH | `@tier(high)` (default) | Garbage collected | Yes | Business logic, I/O |
| MID  | `@tier(mid)`            | Arena allocated    | No  | Parsers, hot paths |
| LOW  | `@tier(low)`            | Manual, borrow-checked | No | Systems, FFI, packets |

Functions without a `@tier` annotation default to HIGH. Tiers are opted
into for performance, not opted out of for convenience.

## Why a tier system

Most languages make one memory bet for an entire program:

| Language | Bet | Cost |
|----------|-----|------|
| Go, Java, C# | GC everywhere | Pauses, GC pressure, heap bloat |
| Rust | Ownership everywhere | Steep learning curve, slow compiles |
| C, C++ | Manual everywhere | Safety bugs, undefined behavior |

Ubel Stratum lets each function make its own bet: garbage collection where
that is the easier choice, arenas where speed matters, manual ownership
where correctness under tight constraints matters most. The compiler
guarantees the bets never clash at runtime, and cross-tier calls follow an
explicit set of rules covered in [The Tier Model](./tier-model.md).

## Current status

Ubel Stratum is early and under active development. The end goal is native
machine code through an LLVM backend; today the language runs on a
tree-walking interpreter, a deliberate stepping stone rather than the
final architecture. Building the interpreter first proves out the
language's semantics completely before the much heavier lift of LLVM
integration begins.

| Phase | Status | Covers |
|-------|--------|--------|
| 1 | Done | Core design, memory model, lexer, arena AST, parser |
| 2 | Done | Semantic analysis: name resolution, type inference, tier enforcement |
| 3 | Done | Tree-walking interpreter, the current execution model |
| 4 | Not started | LLVM backend, native binaries |
| 5 | Not started | Standard library, tooling, package manager |

Source files use the `.ubl` extension. There is no installable compiler
or package manager yet; running Ubel Stratum today means building the
interpreter from source, covered in [Getting Started](./getting-started.md).

The source, issue tracker, and full engineering documentation live at
[github.com/MidManStudio/ubel_stratum](https://github.com/MidManStudio/ubel_stratum).
A [browser playground](/playground/) runs the tokenizer, parser, semantic
analyzer, and interpreter on arbitrary `.ubl` source without installing
anything, and the [CI results](/results/) page tracks the fixture suite
and benchmark history that back every change to the language.
