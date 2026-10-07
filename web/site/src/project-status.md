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
- `type` aliases are transparent: with `type Score = int`, a `Score` and
  an `int` are the same type, with no cast. That holds for aliases to
  primitives, collections, generic struct instances and function types,
  for generic aliases (`type Wrapped<T> = Box<T>`), for an alias of an
  alias, and for an alias declared after its use. An alias to a struct or
  enum also works where the name is built or matched (`P { x = 1 }`,
  `P.origin()`, `C.Red`, patterns), and an alias cycle is an error
- Methods on enums: an `extend` block on an enum adds instance methods and
  associated functions (`Color.make()`), an unknown method on an enum value
  is a compile error, and a struct field of function type can be called
  directly (`config.callback(4)`)
- A type mismatch inside a generic type (`List<int>` against
  `List<string>`) now points at the offending expression instead of the
  top of the file
- Mutable globals: `static NAME: Type = expr` (the type is required, and
  `pub static` exports once modules exist). A static is one value shared
  by every function: assign to it in one function and read it in another.
  It lives in the HIGH tier, so only `@tier(high)` code can read or write
  it, a `const` cannot read it, and a local or parameter of the same name
  shadows it. Pass the value into a MID or LOW function as a parameter
  instead. Initializers run once before `main`, in any declaration order,
  and the declared type types a bare literal (`static HP: u8 = 250`). A
  `@tier(...)` written on a `const` is now an error rather than ignored
- Nested generic arguments (`List<List<int>>`,
  `Dictionary<string, List<int>>`) parse, and annotated dictionaries
  type-check
- A return type annotation on a lambda is one clear parse error instead
  of two unrelated name errors, and an unknown method on a struct is
  reported once instead of twice
- Tooling stubs: a `ubel` command line (`check`, `run`, `debug-tokens`,
  with `--json` output and stable exit codes) and an `ubel-lsp` language
  server that publishes the compiler's diagnostics as you type. Both call
  the same one-function check, so an editor and the command line agree
- Fixed-width integers with real wrapping arithmetic and the full `u64`
  range (`u8`, `i8`, `u16`, `i16`, `u32`, `u64`, and the rest), numeric
  literal suffixes (`255u8`), and a literal-out-of-range diagnostic
- Unsuffixed integer literals take their type from context, the way Rust
  does: `let x: u8 = 250`, a struct field, a call argument, a return
  value, an assignment, the other operand of `+` or `<`, and a `match`
  pattern all give the literal the type the position expects, with the
  same out-of-range check (`let x: u8 = 300` is an error, not a silent
  wrap). A literal with nothing to constrain it stays a plain `int`. An
  integer literal never becomes a float on its own (`let f: float = 5`
  needs `5.0`). Ordering comparisons now work on the fixed-width integers
- A parser ambiguity fix: a bare identifier condition immediately
  followed by a block whose first statement was a plain assignment
  (`if x == y { hit_count = hit_count + 1 }`) could misparse as a
  struct literal; `if`/`while`/`for`/`match` heads no longer read a
  struct literal unless it is parenthesized

## Active work

- The trait system is being built in slices (design in `docs/TRAITS_DESIGN.md`).
  Built: `impl Trait for Type` works and is checked (missing and extra methods,
  signature mismatches, overlap), default methods are inherited or overridden,
  `Self` resolves, an inherent method beats a trait method, two traits
  supplying one name are told apart with `Trait.method(value)`, and a call
  made through a trait runs the trait's method. Trait bounds on generic
  functions, structs and enums are enforced: a call or construction with a
  type that lacks the impl is an error, and a method call on a type parameter
  with no bound is an error. Next: built-in traits, supertraits and generic
  traits, tiers on trait methods, language-item traits, and `dyn Trait`
- Still to decide: whether struct fields get default values. The literal
  typing it depended on has landed, so it can be taken up next
- The language server publishes diagnostics only; hover, go to
  definition and completion are not built

## Known gaps, tracked rather than hidden

- Arguments to built-in collection methods are not type-checked, so
  `list.push(200)` on a `List<u8>` stores a plain `int` and a later
  `list[0] + 100` fails when the program runs
- Format specs for sign, zero padding and numeric base (`{x:+}`,
  `{x:05}`, `{x:x}`) work on plain `int` only, not on the fixed-width
  integers
- Constructing through an alias to a type that is not a struct or enum
  (`type Score = int`, then `Score { x = 1 }`) is not rejected at compile
  time
- `pub` written on a `const` or `type` item, and `@tier(...)` written on
  a `type` item, are parsed and then ignored; constants are global to
  the file and readable from every tier
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
