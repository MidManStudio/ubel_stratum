# ubel_stratum

## Overview

Core compiler crate for Ubel Stratum: the AST definitions, semantic
analysis passes (name resolution, type inference, tier checking, borrow
checking, move checking), the tree-walking interpreter, and error
management. Lexing and parsing live in the separate `ubel_stratum_rd`
crate.

## Modules

### `ast/declarations.rs`

**What it does:** Declaration node types (functions, structs, enums,
traits, impls, parameters).

**Decisions:**
- Added `ParamKind::Discard { ty }` for `_: Type` parameters. Kept
  separate from `ParamKind::Named` rather than reusing `Named` with the
  string `"_"` as a name, since a discarded parameter has no binding at
  all: `_` is its own lexer token, never `Ident("_")`, and there is no
  expression production for a bare `_`, so nothing could ever read it
  back even if it were stored as a name.
- No `default` field on `Discard`. A caller-omittable default is about
  what callers can skip supplying; that is orthogonal to whether the
  callee can read the parameter, so pairing the two did not make sense.

**Tests:** see `sema/tests.rs`, referenced under `sema/name_resolution.rs`
and `sema/type_infer.rs` below.

### `sema/name_resolution.rs`

**What it does:** Pass 1 of semantic analysis. Builds the symbol table
and resolves every name reference to a definition.

**Tests:** `sema/tests.rs`,
`test_sema_discard_param_on_free_function_does_not_report_self_outside_method`.

### `sema/type_infer.rs`

**What it does:** Pass 2 of semantic analysis. Builds the type table,
infers expression types, and checks function signatures against call
sites.

**Tests:** `sema/tests.rs`,
`test_sema_discard_param_type_is_enforced_at_call_site`.

**Decisions (this delivery, `@derive(Eq, Hash, Ord, PartialOrd,
Clone)`, and real `<`/`<=`/`>`/`>=` operators for `Str` and structs):**
- `check_derive_attrs` now recognizes all six derive trait names
  (`PartialEq`, `Eq`, `Hash`, `Ord`, `PartialOrd`, `Clone`), all
  struct-only for now. The five new ones simply aren't implemented for
  `enum` declarations yet, so a request for one on an `enum` is treated
  the same as any other name the function doesn't recognize in that
  context.
- Prerequisite chain, checked directly rather than only transitively:
  `Eq` needs `PartialEq`; `PartialOrd` needs `PartialEq`; `Ord` needs
  both `PartialOrd` and `Eq` (so `@derive(Ord)` alone reports both gaps,
  not just the first one found); `Hash` needs `Eq`. That last one isn't
  a real Rust supertrait bound for `Hash`, but it's the bound every
  actual hash-map API uses in practice, and this project's whole reason
  for wanting `Hash` at all (a future `Dict` key). `Clone` has no
  prerequisite. New error: `TypeError::DeriveRequiresOther`, TYPE-117.
- New `InferCtx::struct_derives: HashMap<DefId, HashSet<String>>`,
  populated in `collect_struct_sig` alongside `check_derive_attrs`'s own
  validation. Sema's own copy, not shared with `Interpreter::
  struct_derives` (type-name-keyed, built later). Each pass re-derives
  this fact from the AST rather than one borrowing the other's table,
  matching the existing relationship between this file's struct tables
  and the interpreter's.
- `.clone()` is now a real, typed instance method on any struct that
  derives `Clone`, resolved as `Self` (the receiver's own type), zero
  arguments. Two separate places needed to know about it, not one: the
  struct-instance-method-call arm (`ExprKind::Call`) computes the actual
  return type, but `ExprKind::Field`'s own struct-field-access handling
  runs *first* whenever a method call's callee gets pre-inferred (the
  same pre-inference this file's own comments already note for
  `boxed.unwrap()`), and that handler's `is_method` check didn't know
  about derive-gated pseudo-methods at all, so it reported `NoSuchField`
  before the `Call` arm's dispatch ever ran. Fixed by teaching that
  check about `Clone`'s `.clone()` too, not just `struct_methods`.
- `binop_result`'s `Lt`/`Le`/`Gt`/`Ge` arm used to be folded in with
  `Eq`/`Ne`, unifying operand types and calling it done, with no check
  that the resulting type was actually orderable at all. Split out on
  its own now: orderable means `Int`/`Float`/`Double` (unchanged), `Str`
  (new), or a struct that's derived `PartialOrd`/`Ord` (new). New error:
  `TypeError::TypeNotOrderable`, TYPE-118. `Bool` used to reach the
  exact same runtime panic `Str` did (`eval_binop`'s `promote_numeric`,
  "arithmetic not supported on bool"); it now fails here instead, at
  compile time, with a real message. That isn't new scope, just the
  same underlying always-broken case getting a proper diagnosis instead
  of a crash.

**Tests:** the four new fixtures under `err_derive_missing_prerequisite_*`
and `ok_derive_ord_and_clone_*` / `ok_clone_deep_vs_shared_alias_*`
exercise the sema side end to end; `interpreter/value.rs`'s own test
module covers the `Value`-level comparison/hash/clone semantics these
checks gate.

**Decisions (docs/PRINT_FORMAT_RULES.md §4 leftovers):**
- `check_format_spec` extended: sign forcing and zero-padding are
  restricted to `Int`/`Float`/`Double` (same `TYPE-115` family
  `.precision` already used), a numeric base is restricted to `Int`
  alone (`Float`/`Double`/`Str` don't have a meaningful "value in hex"),
  and the alternate form is rejected outright when no base is present
  (new `TypeError::AlternateFormatWithoutBase`, `TYPE-119`: a
  spec-internal combination problem, not a wrong-type one, so it's its
  own variant rather than another `InvalidFormatSpec` case).
- `?` and a trailing base are mutually exclusive, but that check lives
  in the parser (`rd_parser`'s `parse_format_trailer`), not here:
  `Int`, the only type a base ever applies to, never diverges between
  `Display` and `debug_string` in the first place, so there's nothing
  for sema to type-check; the combination is simply never grammatically
  valid to begin with.

### `interpreter/value.rs`

**What it does:** Runtime `Value` representation and its core
operations: `equals`, `debug_string`, `Display`, and, as of this
delivery, `partial_cmp`, `compute_hash`, and `deep_clone`.

**Decisions:**
- `Value::Struct` gained four fields: `derives_ord`, `derives_hash`,
  `derives_clone` (booleans, same construction-time-resolved pattern
  `derives_partial_eq` already established), and `field_order:
  Rc<Vec<String>>`, field names in declaration order. `fields` itself
  is a `HashMap`, which has no defined iteration order and isn't
  guaranteed to agree between two separately-constructed instances of
  the same type, so a well-defined `partial_cmp` (field order changes
  the actual comparison result, not just the iteration) and a
  *consistent* `compute_hash` (two structurally-equal instances must
  hash equal, which an arbitrary per-`HashMap` bucket order can't
  promise) both needed a real, shared, declaration-derived order.
  Populated unconditionally for every named struct, not gated on the
  derives themselves. It costs one `Rc<Vec<String>>` clone either way,
  and unconditional population is one code path instead of two.
- `partial_cmp`: `Unique`/`Shared`/`SyncShared` all delegate to the
  *inner* value's ordering. Confirmed, and a deliberate divergence from
  `equals()` for `Shared`/`SyncShared` specifically, which compare by
  `Rc::ptr_eq`. Ordering two `Shared<T>` values by raw pointer address
  would be legal Rust but meaningless to whoever wrote `a < b` (and
  non-deterministic run-to-run besides), so content is the only choice
  actually useful for sorting, unlike equality, where "same object" is
  a meaningful question on its own.
- `compute_hash`: checked variant-by-variant against `equals()`'s own
  rule for that variant, not assumed. Structural-in-`equals()` variants
  (`Struct` when derived, `Tuple`, `Unique`, `Enum` always) hash
  structurally; ptr-eq-in-`equals()` variants (`List`/`Dict`/`Queue`/
  `Stack`/`Pool`/`InlineList`/`Linqerizer`/`Shared`/`SyncShared`, and a
  non-derived `Struct`) hash the `Rc` pointer address instead of
  contents. Hashing contents there would let two *unequal* (by
  `equals()`) same-valued instances collide into looking
  interchangeable, and would break the moment either mutated after
  insertion. `Float`/`Double` hash by bit pattern with `-0.0`
  normalized to `0.0`'s bits first (`equals()`'s plain `==` says `-0.0
  == 0.0`; without normalizing, that pair would still hash unequal).
  `NaN` gets no such treatment: `equals()` already says `NaN != NaN`,
  so two `NaN`s aren't required to hash equal. `EnumPayload::Struct`
  sorts its field map by name at hash time instead of needing its own
  `field_order`-style plumbing, since enum `@derive` isn't in scope this
  delivery, and hash order only needs to be consistent, not meaningful
  to a reader the way `Struct`'s declaration order needs to be for
  `partial_cmp`. No consumer yet (`Dict` is still `Vec<pair>`, and
  `Value` having no `Hash` is why, per this file's own top doc comment),
  shipped ahead of one anyway, same precedent `move_facts.rs` set in
  an earlier delivery: real behavior, real unit tests, nothing
  user-observable different until something downstream consumes it.
- `deep_clone`: recurses through everything a struct might hold
  *except* `Shared`/`SyncShared`, which alias (bump the `Rc`) instead.
  The whole reason those two wrappers exist is deliberate, explicit
  shared ownership, so recursing through one would silently undo the
  one thing the person wrote `Shared<T>` to ask for; matches
  `Rc<RefCell<T>>::clone()` in real Rust for the same reason. `Pool`/
  `InlineList`/`Linqerizer` are a stated, known limitation: deep-cloning
  a generational slot table or a lazy pipeline's snapshot correctly is
  real, separate design work with no motivating use case yet, so they
  fall back to the same shallow `Rc`-bump the derived Rust `Clone`
  already gives every `Value` for free.

**Tests:** `interpreter/value.rs`'s own `#[cfg(test)]` module gained 10
new tests, covering numeric/`Str`/`Bool` `partial_cmp`, `NaN`
incomparability, struct comparison respecting declaration order (not
alphabetical), non-derived structs being incomparable, all three
wrapper types delegating to their inner value, hash/equals agreement
for a derived struct, `-0.0`/`0.0` hashing equal, a non-derived
struct's identity-based hash, and `deep_clone`'s two divergent cases
(a `List` field genuinely independent after cloning; a `Shared` field
still the same `Rc`).

### `interpreter/eval/expr.rs`

**What it does:** Expression evaluation: binary/unary operators,
method calls, struct/anon-object construction.

**Decisions:**
- `eval_binop`'s `Lt`/`Le`/`Gt`/`Ge` used to go straight to
  `promote_numeric`, which only handles `Int`/`Float`/`Double`; `Str`,
  `Struct`, and anything else reached a runtime panic
  ("arithmetic not supported on {type}"). Added a new match arm ahead of
  the numeric path, gated on either operand being `Str`/`Struct`/
  `Unique`/`Shared`/`SyncShared`, that calls `Value::partial_cmp`
  directly. The existing numeric path is untouched, not folded into
  `partial_cmp` here, since it already works and this delivery's job
  didn't include touching it. `TYPE-118` has already gated this at sema
  time for well-formed programs, so `None` from `partial_cmp` here (an
  interpreter-only test that skips sema, or a genuinely incomparable
  runtime pair) becomes a panic, not a silent wrong answer.
- `eval_method_call`'s struct dispatch: a user-defined `method_table`
  entry is still checked first (an explicit `fn clone(&self)` a person
  writes themselves wins), and only once that lookup misses does
  `.clone()` get checked against `derives_clone`, calling
  `Value::deep_clone`. Mirrors sema's own resolution order for the
  same reason.
- `apply_format_spec` (docs/PRINT_FORMAT_RULES.md §4 leftovers): a
  numeric base (`x`/`X`/`o`/`b`) builds its own sign/prefix/zero-pad/
  digits string directly (`render_int_with_base`) rather than reusing
  the general path, since those three all need to land between each
  other in a specific order (sign, alternate-form prefix, zero-fill,
  then digits, e.g. `-0x00ff`) that the general decimal path never
  had to think about. A negative `Int` renders as its 64-bit two's-
  complement bit pattern in the chosen base, matching Rust's own
  `{:x}` on a signed integer, not a `-` sign plus the magnitude's
  digits. Zero-padding ignores `align`/`fill` entirely when both are
  present (the well-established convention this feature is modeled
  on, Rust's own `format!`, does the same). It always pads
  immediately before the digits, after any sign and prefix.

**Tests:** the four new fixtures under `ok_format_spec_extended_*` and
`ok_format_spec_numeric_base_*` / `err_format_spec_*` exercise all of
this end to end. No unit tests for this function specifically, same
precedent the original precision-only version of this feature already
set (fixture-tested only).

### `lexer/logos_lexer.rs`

**What it does:** The actual token generator, a `logos`-derived enum
(`LogosToken`) mapped onto the public `TokenType` the rest of the
compiler sees.

**Decisions:**
- New `#[token("#")] Hash` variant, modeled directly on the adjacent
  `At` (`@`) token. `#` was not a lexable character in this language
  at all before this delivery. Needed for the alternate-form flag in
  a format spec (docs/PRINT_FORMAT_RULES.md §4).

### `lexer/token.rs`

**What it does:** The public `TokenType` enum every other crate sees,
plus its `Display` impl (used for "expected X, found Y" parser error
messages).

**Decisions:**
- `Hash` added alongside `At` in both the enum and the `Display` impl
  (`write!(f, "#")`), completing the new token from `logos_lexer.rs`
  above. `cargo build` catches a missing enum variant everywhere a
  `match` isn't already using a wildcard arm; it does not catch a
  missing `Display` arm on its own (that one only fails to render
  correctly, it doesn't fail to compile), so it was checked directly
  rather than assumed covered by the exhaustiveness check that did
  cover everything else.

### `ast/literals.rs`

**What it does:** Literal AST nodes, including `FormatSpec`.

**Decisions:**
- `FormatSpec` gained `fill`, `sign_plus`, `alternate`, `zero_pad`, and
  `base` (a new `NumericBase` enum: `Hex`/`HexUpper`/`Octal`/`Binary`),
  completing docs/PRINT_FORMAT_RULES.md §4's four remaining items
  (fill character, sign forcing, the alternate form and zero-padding,
  and numeric bases). `fill` is `Option<char>` but only ever
  meaningful alongside `align`. The parser (`rd_parser`'s
  `parse_format_spec`) never produces one without the other, so
  nothing downstream needs to separately guard against that
  combination.

### `interpreter/eval/mod.rs`

**What it does:** The `Interpreter` struct and its top-level program
registration: function and method tables, the struct-derive table, and
the driver loop that walks the parsed program before execution starts.

**Decisions:**
- `register_fn`/`register_method` now build their `params: Vec<String>`
  list with `enumerate()` and a synthesized `$discardN` name for each
  `ParamKind::Discard` slot, instead of dropping it. That list is later
  zipped positionally against real call arguments, so dropping a slot
  would shift every later argument onto the wrong parameter name. The
  `$` prefix cannot collide with a real identifier, since identifiers
  cannot start with `$`.
- New `struct_field_order: HashMap<String, Rc<Vec<String>>>`, built in
  the same pre-declare pass as `struct_derives`, from the same
  `StructDecl` walk. Re-derives an already-available fact (field
  declaration order) rather than reading back through sema's own
  `struct_fields` table, which is `DefId`-keyed and not otherwise
  shared with the interpreter. See `Value::Struct::field_order`'s own
  doc comment (`interpreter/value.rs`) for why this matters for
  `partial_cmp`/`compute_hash` specifically.

### `error_management/errors/types/mod.rs`

**What it does:** `TypeError`, errors from ordinary type inference and
type checking (TYPE-1xx range).

**Decisions:**
- `TypeError::DeriveRequiresOther` (TYPE-117) and `TypeError::
  TypeNotOrderable` (TYPE-118), both new this delivery. See
  `sema/type_infer.rs` above for what triggers each. Kept as two
  separate variants from `UnknownDeriveTrait` (TYPE-116) on purpose:
  neither an incomplete-but-valid derive request nor a real orderability
  failure is "unknown" in the sense that variant's own doc comment
  means.
- `TypeError::AlternateFormatWithoutBase` (TYPE-119): `#` in a format
  spec with no trailing base. Kept separate from `InvalidFormatSpec`
  (TYPE-115) for the same reason as the two above: this isn't `value`
  having the wrong type, it's the spec itself missing a piece it
  depends on.

### `builtins/instance/linqerizer_methods.rs`

**What it does:** Instance methods on `Value::Linqerizer`: `.order_by()`,
`.group_by()`, `.select()`, `.where()`, and the rest of the lazy pipeline.

**Decisions:**
- Retired this file's own private `compare_values` (`Int`/`Float`/
  `Double`/`Str`/`Bool` only) in favor of calling `Value::partial_cmp`
  directly from `.order_by()`/`.order_by_desc()`'s `materialize` step,
  the same single-source-of-truth relationship every other comparison
  in the interpreter already has with `Value::equals`. A struct with a
  derived ordering now sorts correctly through `.order_by()` too, as a
  direct consequence of reusing the general implementation rather than
  something built specifically for this call site.

## CI and Workflows

- `.github/workflows/ci-check.yml`, "Ubel Stratum, Fast Compile Check":
  runs on every push and pull request to `master`/`main`.
- `.github/workflows/pipeline-dashboard.yml`: builds the full
  tokenize, parse, sema, interpret diagnostic report plus benchmarks
  for every fixture in `tests/fixtures`, publishes as a static site via
  GitHub Pages. Kept separate from `ci-check.yml` since the benchmark
  step is slow and this produces a deployable artifact rather than a
  pass/fail gate.
- `.github/workflows/run-replacements.yml`: applies a `.mdix/replacements`
  bundle to the repo, dry run only unless manually dispatched with
  `dry_run: false`.

## Fixes and Problems

### `sema/name_resolution.rs`

- `resolve_param`'s catch-all arm was written with only the `self`
  family of `ParamKind` variants in mind: anything that was not
  `Named` fell into a check for `NameError::SelfOutsideMethod`. Once
  `ParamKind::Discard` was added, a `_: Type` parameter on a plain free
  function incorrectly triggered that error. Fixed by giving `Discard`
  its own arm ahead of the `self`-family catch-all.

### `sema/type_infer.rs`

- Three separate signature-collection call sites used
  `ParamKind::Named { ty, .. } => ty.map(...), _ => None` with
  `filter_map`, which silently dropped a `Discard` parameter's declared
  type from the function's computed signature entirely, not merely its
  name. This meant the parameter's type was never checked against call
  sites and the effective arity used for type checking was one short
  per discarded parameter. Fixed by matching `Named` and `Discard`
  together, since both contribute a real type to the signature; only
  `self` variants are correctly excluded.
- (this delivery) First pass at `binop_result`'s new orderability check
  flagged `SemaType::Unknown`, a genuinely not-yet-resolved type
  variable, not a known-bad one, as "not orderable". This fired for
  real inside a `Linqerizer` lambda body (`ok_linqerizer_group_by.ubl`'s
  `.where(fn(i) i.name.len() > 5)`): the lambda parameter's type is
  still pending unification with the pipeline's element type at the
  point that particular `>` gets visited, so `self.apply(lhs)` resolved
  to `Unknown` rather than the `Int` it would settle on moments later.
  Not caught by writing the check, caught by running the full fixture
  sweep afterward and finding this one `ok_` fixture had regressed to a
  sema failure it shouldn't have had. Fixed by treating
  `SemaType::Unknown` as orderable. "Don't know yet" isn't "known to
  be wrong", and no other check in this file treats `Unknown` as a
  positive finding of its own either.
- (this delivery) `.clone()` dispatch needed fixing twice, not once,
  both found by actually running the new
  `ok_derive_ord_and_clone_isolated.ubl` fixture rather than by
  inspection alone:
  - First: `ExprKind::Field`'s own `is_method` check (used to decide
    whether a name that isn't a real field might still be a method,
    before reporting `NoSuchField`) didn't know about derive-gated
    pseudo-methods at all, so `original.clone()` failed with
    `NoSuchField` before the `Call` arm's own instance-method dispatch,
    which *did* already know about `Clone`, ever got a chance to
    run. `.clone()`'s callee gets pre-inferred as a bare `Field`
    expression first, the same pre-inference this file's comments
    already note happens for `boxed.unwrap()`.
  - Second, once the first fix was in place: a plain `field == "clone"`
    comparison failed to compile. This function matches on `&expr.kind`
    (a reference), so under Rust's default binding modes `field` binds
    as `&&str`, not `&str`. Every other `field == "..."` comparison in
    this file lives inside a *different* pattern (`if let
    ExprKind::Field { .. } = callee.kind`, matching an owned, `Copy`
    value), which is why the existing code never needed the extra
    deref. Fixed with `*field == "clone"`.

### `interpreter/eval/mod.rs`

- `register_fn` and `register_method` built their parameter name list
  with `filter_map`, dropping `Discard` slots the same way the
  `type_infer.rs` sites did. Since that list is zipped positionally
  against call arguments at call time, a dropped slot shifted every
  later argument onto the wrong parameter name, silently binding the
  wrong value. Fixed with a synthesized per-position placeholder name
  instead of dropping the slot; see the Decisions note above.

### `tests/string_interpolation_test.rs`

- This file, along with three siblings, sat in a root-level `tests/`
  directory with no package of its own, so `cargo test --workspace`
  never ran any of it (housekeeping item on the roadmap). Moving
  `basic_tokens_test.rs`, `comment_test.rs`, and `error_recovery_test.rs`
  into `crates/core/tests/` (where Cargo auto-discovers them as real
  integration tests) needed no code changes at all, confirmed by running
  them, not assumed. This file needed a real fix: `InterpolationPart::
  Expr` used to hold the hole's raw source text (`String`) and at some
  point since these tests were written became a pre-tokenized
  `Vec<Token>` instead, so every assertion comparing a hole's contents
  against a source string no longer compiled. Rewrote every affected
  assertion to check actual token kinds instead, verified empirically
  against the real lexer output for each case (including the nested-
  braces case, `arr[{idx}]`, confirmed still handled correctly by the
  existing brace-depth counting) rather than guessed.
- Two more root-level test files turned up during the same housekeeping
  pass, neither in the roadmap's original list of four, handled
  differently on purpose rather than uniformly: `tests/lexer/
  keywords_test.rs` was confirmed (via `diff`, not assumed) to be a
  strict subset of `basic_tokens_test.rs` with zero unique coverage, so
  it was deleted rather than also wired in. `tests/sema/
  name_resolution_tests.rs` references `ubel_stratum::parser` and
  `SemaType::contains_arena_ref`, neither of which exist anymore (looks
  like it predates the parser's extraction into its own crate), a real
  port, not a wiring job, so it was left alone rather than either fixed
  unasked or deleted outright.

## Documentation convention: scope note

`interpreter/value.rs`, `interpreter/eval/mod.rs`, `interpreter/eval/expr.rs`,
`sema/type_infer.rs`, `error_management/errors/types/mod.rs`,
`builtins/instance/linqerizer_methods.rs`, `lexer/logos_lexer.rs`,
`lexer/token.rs`, `ast/literals.rs`, `crates/rd_parser/src/parsers/
parse_expr.rs`, and the four relocated `crates/core/tests/*.rs` files
were touched across the two deliveries this session (`@derive`, then
the print-format leftovers), so all of them got NOTICE headers, and
every line actually added or rewritten follows the style rules (no em
dashes, no first/second person), checked twice for the first delivery,
not once: a few lines were missed on the initial pass and only caught
while writing this delivery's own documentation, then fixed
retroactively rather than left standing. What did not happen: a
retroactive sweep of each file's full pre-existing comment history.
Several of these files predate DOCUMENTATION_AND_COMMENTING_GUIDELINES.md
by multiple earlier sessions and carry hundreds of pre-existing em dashes
each (`type_infer.rs` alone has well over a hundred). Rewriting all of
that was not part of either delivery's actual work and risks introducing
real bugs by touching thousands of unrelated lines under time pressure,
for a purely cosmetic gain. Same incremental principle the guidelines
file itself states for the documentation split: real, separate future
work if
wanted, not assumed, ask first.
