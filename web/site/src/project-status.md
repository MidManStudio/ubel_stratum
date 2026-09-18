# Project Status

Ubel Stratum is early and under active development. This page tracks
what exists today at a language level; the [CI Results](/results/) page
tracks the fixture suite and benchmark history behind these claims.

## Phases

| Phase | Status | Covers |
|-------|--------|--------|
| 1 | Done | Core design, memory model, lexer, arena AST, the recursive-descent/Pratt parser |
| 2 | Done | Semantic analysis: name resolution, type inference, tier enforcement, generics, the three-tier escape-boundary checker |
| 3 | Done | Tree-walking interpreter, the current execution model |
| 4 | Not started | LLVM backend, native binaries |
| 5 | Not started | Standard library, tooling, package manager |

Phase 2 being marked done covers full generics for structs and enums,
enum discriminants and payloads, arena and pool escape checking, and
generational handles through `Pool<T>`/`Handle<T>`. It does not mean
every corner of semantic analysis is finished: LOW-tier borrow checking
has real, CFG-based loan and liveness enforcement in place (genuinely
non-lexical — a borrow's last actual use determines when it stops
conflicting, not the enclosing block) and move checking alongside it,
but that checking is intra-function only. Declared lifetime parameters
(`[lifetime L]`) parse and are checked for internal well-formedness, but
nothing yet verifies that a reference crossing a function call or
`edge struct` field actually respects the declared relationship — see
[The Tier Model](./tier-model.md#what-is-enforced-today) for the exact
line between what parses and type-checks versus what is actually
verified safe.

## Recently landed

- `_` as both a match wildcard and a parameter placeholder
- `@derive` for `PartialEq`, with `Eq`, `Hash`, `Ord`/`PartialOrd`, and
  `Clone` following
- Lifetime well-formedness checking: declared lifetime names must exist,
  `where` clause bounds can only reference declared names, no outlives
  cycles, for both function signatures and `edge struct` fields
- Method dispatch through `Unique<T>`/`Shared<T>`/`SyncShared<T>`
  ownership wrappers
- A parser ambiguity fix: a bare identifier condition immediately
  followed by a block whose first statement was a plain assignment
  (`if x == y { hit_count = hit_count + 1 }`) could misparse as a
  struct literal; `if`/`while`/`for`/`match` heads no longer read a
  struct literal unless it is parenthesized

## Active work

- Connecting `edge struct`'s `is_edge` marker to the arena-escape
  checker, its documented purpose today has no effect on that checker
- Outlives/subset enforcement across a function or `edge struct`
  boundary — the internal groundwork (materializing what a loan's own
  valid range actually is) has landed; the checks that use it at an
  actual call site or struct construction have not yet

## Known gaps, tracked rather than hidden

- `edge struct` is parsed and stored on the AST but not yet consulted by
  the arena-escape checker, so it does not yet do what its name implies
- The interpreter runs every tier on the same reference-counted values;
  `with arena` blocks are validated by the tier checker but do not yet
  allocate or free real memory, that lands with the LLVM backend
- No package manager, no installable compiler, no standard library
  beyond the built-in collection and instance methods documented in the
  [Language Tour](./language-tour.md)

## Source and deeper documentation

The [GitHub repository](https://github.com/MidManStudio/ubel_stratum)
carries the full engineering documentation this site draws from:
`MEMORY_MODEL.md`, `PARSER_RULES.md`, `DIAGNOSTICS_RULES.md`,
`ENUM_RULES.md`, `GENERICS_RULES.md`, `DATASTRUCTURES.md`, and
`PARKED_IDEAS.md` among others, each maintained alongside the code it
describes rather than as a separate, drifting reference.
