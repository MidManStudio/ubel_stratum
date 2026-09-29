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
but the loan and move checking itself is intra-function. Declared
lifetime parameters (`[lifetime L]`) are checked for well-formedness,
and outlives enforcement verifies that a reference crossing a function
call or an `edge struct` field actually respects the declared
relationship, with a few documented scope limits (methods, references
nested inside generics, closures capturing references) — see
[The Tier Model](./tier-model.md#what-is-enforced-today) for the exact
line between what parses and type-checks versus what is actually
verified safe.

## Recently landed

- `_` as both a match wildcard and a parameter placeholder
- `@derive` for `PartialEq`, with `Eq`, `Hash`, `Ord`/`PartialOrd`, and
  `Clone` following
- Outlives/subset enforcement: lifetime well-formedness (declared names
  must exist, `where` bounds reference declared names, no outlives
  cycles), plus boundary checks at a function call and at `edge struct`
  construction, with multi-lifetime constraint propagation
- `edge struct` now participates in arena-escape checking: a named
  reference field on an `edge struct` is checked against its declared
  lifetime rather than the general arena boundary rule
- Method dispatch on user-defined structs through `extend` and inherent
  `impl` blocks, both instance methods and `Type.method()` static calls,
  with `self` now type-checked inside those bodies
- Method dispatch through `Unique<T>`/`Shared<T>`/`SyncShared<T>`
  ownership wrappers, for builtin collections and for user-defined
  struct methods, and calling a user-declared method on a `Unique<T>`
  local no longer counts as moving it, so several calls in a row are
  accepted while a genuine second move is still rejected
- Global `const` items are evaluated before `main` runs, in any
  declaration order, and assigning to one is a compile error
- Nested generic arguments (`List<List<int>>`,
  `Dictionary<string, List<int>>`) parse, and annotated dictionaries
  type-check
- A return type annotation on a lambda is one clear parse error instead
  of two unrelated name errors, and an unknown method on a struct is
  reported once instead of twice
- Fixed-width integers with real wrapping arithmetic and the full `u64`
  range (`u8`, `i8`, `u16`, `i16`, `u32`, `u64`, and the rest), numeric
  literal suffixes (`255u8`), and a literal-out-of-range diagnostic
- A parser ambiguity fix: a bare identifier condition immediately
  followed by a block whose first statement was a plain assignment
  (`if x == y { hit_count = hit_count + 1 }`) could misparse as a
  struct literal; `if`/`while`/`for`/`match` heads no longer read a
  struct literal unless it is parenthesized

## Active work

- The trait system: `trait` declarations and `impl Trait for Type`
  blocks parse, but trait method dispatch, `dyn Trait`, and bound
  enforcement are not built and still need a design pass
- Design decisions pending before they are built: how a bare integer
  literal should meet a sized-integer field, whether struct fields get
  default values, and what a mutable global looks like across tiers

## Known gaps, tracked rather than hidden

- An unsuffixed integer literal does not coerce to a sized-integer
  field or binding (`let x: u32 = 10` needs `10u32`)
- `type` aliases are not yet transparent: with `type Score = int`, a
  `Score` and an `int` do not unify without an explicit cast
- An unknown method called on an enum value is not caught at compile
  time and panics when the program runs
- Calling a function stored in a struct field (`config.callback(4)`) is
  reported as an unknown method
- `pub` and `@tier(...)` written on a `const` or `type` item are parsed
  and then ignored; constants are global to the file and readable from
  every tier
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
