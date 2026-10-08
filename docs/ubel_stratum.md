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
- New `StaticDecl { attributes, visibility, name, ty, value, span }` and
  `Item::Static` for `static NAME: T = expr`. `ty` is a required
  `&Type`, not an `Option` like `ConstDecl::ty`: the parser rejects a
  static with no annotation. `StaticDecl` carries `visibility`, which
  `ConstDecl` still does not. It carries no tier: a static is always HIGH,
  and the only tier that can exist is an own `@tier(...)` attribute, which
  the parser checks.

**Tests:** see `sema/tests.rs`, referenced under `sema/name_resolution.rs`
and `sema/type_infer.rs` below.

### `sema/name_resolution.rs`

**What it does:** Pass 1 of semantic analysis. Builds the symbol table
and resolves every name reference to a definition.

**Decisions (`static` items):**
- New `DefKind::Static` (`symbol_table.rs`), declared top-level with the
  item's own `visibility`, so `pub static` is `Public` and the default is
  `Private`. It is not `DefKind::Const`, so the `NAME-007` assignment check
  does not apply: a static is assignable.
- `Resolver::in_const_initializer` is set while a `const` initializer is
  resolved. `resolve_name` reports `NAME-008` (`StaticInConst`) when an
  identifier inside it resolves to a `DefKind::Static`. A static
  initializer is resolved with the flag clear, so it may read constants and
  other statics.

**Tests:** `sema/tests.rs`,
`test_sema_discard_param_on_free_function_does_not_report_self_outside_method`;
`crates/rd_parser/tests/statics.rs` for `static`.

### `sema/type_infer.rs`

**What it does:** Pass 2 of semantic analysis. Builds the type table,
infers expression types, and checks function signatures against call
sites.

**Tests:** `sema/tests.rs`,
`test_sema_discard_param_type_is_enforced_at_call_site`.

**Decisions (context-driven typing of unsuffixed integer literals):**
- An unsuffixed `Literal::Int` is a fresh type variable that remembers
  its value and span (`infer_int_literal`; `int_lit_vars` maps the
  variable number to its `IntLitUse` list, `int_lit_sites` lists every
  literal site seen). `unify` calls `try_unify_int_lit` right after
  resolving both sides: a concrete integer type range-checks every
  literal behind the variable (`TYPE-120`, `bind_int_lit`) and binds it;
  two literal variables merge; an ordinary variable takes the literal's
  type; `Unknown` absorbs; an `Optional<T>` unifies its payload with the
  literal; anything else settles the literal as `int` and unifies again
  so the mismatch reads the way it did before. `int_type_range` is the
  one table of integer types, their ranges and display names, and
  deliberately excludes `float`/`double`.
- A literal directly under unary `-` goes through `infer_int_literal`
  with the negated value and records the operand span, so the type's
  most-negative value is reachable and `-1` for an unsigned type is
  rejected.
- `finish_int_literals` runs at the end of each function body, method
  body and const initializer. It settles anything still open as plain
  `int` and records the final width of each literal in
  `SemaContext::int_literal_types`, but only for widths the interpreter
  represents differently from plain `int`. `default_if_int_lit` settles
  one variable on the spot for the places that need a concrete shape
  now (method receivers, `await`, `unwrap_reference`, and
  type-dependent format specs). It returns the resolved type, never the
  stale id of an already-settled variable.
- `check_format_spec` settles an open literal only for a spec that
  depends on the type (precision, `+`, zero-pad, a numeric base), since
  the runtime implements those on plain `int` only. Width, fill, align
  and `?` leave it open.
- `binop_result`'s orderable list now includes the sized integers and
  `f32`/`f64`, and an open literal variable, so `a < b` on two `u32`
  values is no longer `TYPE-118`.
- `display_type` shows an open literal variable as `{integer}`.

**Decisions (traits, slice S1a):**
- `traits: HashMap<DefId, TraitInfo>` is filled by `collect_trait_info`:
  every required signature and default method, written against the abstract
  `Self`, the reserved `Param` whose index equals the trait's generic arity.
  `register_trait_impls` checks each `impl Trait for Type` against it
  (`TYPE-122` to `TYPE-129`) and registers the impl's methods, and the
  defaults it does not override, into `struct_methods`.
- `add_trait_method` keeps one table entry per `(type, name)`: an inherent
  method wins (`trait_entries` records which entries a trait put there), and a
  second trait supplying the name marks it in `ambiguous_methods`.
  `method_origins` lists the supplying traits for the message.
- `self_type` is what the written type `Self` means (the implementing type, or
  the abstract `Self` in a trait); `current_bounds` maps a type parameter to
  its trait bounds (today only the abstract `Self`, bounded by its own trait
  in a default body). `call_through_bounds` resolves `x.m()` on such a
  receiver through the bounds and records the callee span in
  `SemaContext::trait_call_sites`. `infer_qualified_trait_call` handles
  `Trait.m(value, ..)`.
- `register_generic_arities` now runs unconditionally first, and
  `target_type_def_id` follows a type alias.

**Decisions (traits, slice S1b):**
- `register_generic_bounds` runs right after `register_generic_arities`. It
  validates every bound name once per declaration (`TYPE-122`, or `TYPE-129`
  for `Hash`, a type alias parameter and a method's own generic parameter)
  and records the resolved bounds in `generic_bounds`, keyed by the
  function, struct or enum and then the parameter position. The written
  parameter names go in `generic_param_names`.
- `push_generic_scope` returns a `GenericScope` (names and bounds) and fills
  `current_bounds` from the parameters' bound names, so the bounds follow the
  names. A body that does not push its declaration's scope (a generic function
  body, an `extend` or `impl` body on a generic struct) calls
  `install_def_scope`, which sets `current_bounds` and the display names in
  `param_names` without making the names resolvable as types.
- A method call whose receiver (wrappers peeled by `peel_wrappers`) is a type
  parameter goes through `call_through_bounds` when bounds are in force and is
  `TYPE-130` when none are.
- `Obligation { ty, trait_def, span }` is raised by `require_bound` where a
  bounded parameter is instantiated: `call_return_type` (the callee's bounds
  are found through `fn_type_defs`, so a bounded function held in a variable
  keeps them), `instantiate` (a struct literal or enum variant, which now takes
  the span), the associated-function call on a generic struct, and
  `ast_type_to_sema` for an annotation naming a bounded struct or enum.
  `bound_status` answers yes, no or not yet; a built-in type answers no, a
  type parameter is looked up in `current_bounds`, a named type in
  `trait_impls` once `impls_registered`, an unresolved variable stays
  pending. `check_obligations` runs from `finish_body_checks` after integer
  literals are settled and reports in span order; signature-time obligations
  that had to wait are flushed at the end of `collect_signatures`.

**Decisions (type aliases, enum and field calls, nested spans):**
- `collect_alias_sigs` expands every `type` alias to a fixpoint before any
  other signature is collected (`alias_expansions: HashMap<DefId,
  AliasExpansion { arity, target }>`, `alias_prepass`, `alias_blocked`).
  `ast_type_to_sema`'s `Named` arm calls `expand_alias`, which substitutes
  the use site's generic arguments into the stored target (written with
  `Param(i)` for the alias's own generics), so an alias is replaced by its
  target and is never a type of its own. No progress in a round means a
  cycle: `TYPE-121` once per remaining alias, expanding to an unknown type.
- `type_def_of(name)` is the lookup for a type NAME in expression and
  pattern position (struct literal, associated call, enum variant path); it
  follows an alias to a struct or enum definition. Six sites use it instead
  of `top_level_def`.
- The instance-method branch of the `Call` arm treats a user struct OR enum
  as "has a possibly empty methods list" (an enum only has an entry when an
  `extend` adds a method, which is why an unknown method on a plain enum
  used to fall through unchecked). A struct field named like the called
  member and not shadowed by a method is called as a field
  (`calls_a_field`): the call is typed by `call_return_type` on the field's
  function type.
- `has_static_method` exempts an associated function from `UnknownVariant`
  in both the `Call` arm and the callee `Field` arm; the callee arm also
  stays silent when it is a callee, so `Color.nothing()` is one report.
- `structurally_compatible` takes the `span` of the outer `unify` and uses
  it for every nested unify. Its six `Span::at(0)` are gone.

**Decisions (`static` items):**
- `collect_static_sig` records the declared type before any body is
  inferred (the parser guarantees one exists), and `infer_static_body`
  unifies the initializer with it and then calls `finish_int_literals`,
  exactly as a const does, so `static HP: u8 = 250` types its literal as a
  `u8`. Assignments to a static go through the ordinary `Assign` path, so
  the declared type is enforced at every one of them.

**Tests:** `crates/rd_parser/tests/int_literal_typing.rs`, the eight
`ok_int_literal_*` and five `err_int_literal_*` fixtures under
`tests/fixtures/`, and two end-to-end cases in
`crates/cli/tests/cli_tests.rs`.

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

**Decisions (Open Decision #6, docs/MEMORY_MODEL.md §12, resolved):**
- `collect_fn_sig`/`collect_method_sig` used to resolve each param's
  type annotation (`ast_type_to_sema`) purely to build the function's
  own aggregate `SemaType::Function` for call-site checking, and threw
  the per-param result away otherwise. `seed_param` then resolved the
  exact same AST node again, independently, later, to seed the param's
  binding type for body-checking. New `seed_param_type` helper: both
  `collect_fn_sig` and `collect_method_sig` now call it right after
  resolving each param, recording that `TypeId` into
  `SemaContext::binding_types` (keyed by the param's own span, a new
  `binding_type` getter added alongside the existing `set_binding_type`)
  and into `def_types` via the param's own `DefId`, exactly what
  `seed_param` used to do itself. `seed_param` now checks
  `binding_type` first and returns early if it's already set, falling
  back to its old resolve-from-scratch behavior only for the params
  that never went through signature collection at all (there currently
  are none in practice, but this keeps the function correct rather than
  assuming that always holds).
- Confirmed empirically before touching anything, not assumed from the
  doc note that first flagged this: the bug isn't only a duplicate
  diagnostic. `collect_fn_sig` pushes the function's own generic scope
  before resolving param types; `seed_param`'s independent second
  resolution never had that scope pushed anywhere in its own call
  chain. For a free function's own generic param (`fn identity<T>(x:
  T)`), that meant the body-checking side silently treated `x` as an
  unconstrained type instead of the real `Param(0)` placeholder: `let
  y: int = x` type-checked with zero errors before this fix. Threading
  the signature-collection result through fixes both problems at once,
  since it removes the second, scope-less resolution entirely rather
  than just deduplicating whatever diagnostic it happened to produce.
- Checked the method case specifically before calling this done: inline
  struct methods were never affected by the generic-scope half of the
  bug, since `collect_struct_sig`/`infer_struct_bodies` already push the
  struct's generic scope once, around both `collect_method_sig` and
  `infer_method_body` together, so both phases already agreed. The fix
  is a pure efficiency and diagnostic-count win there, not a behavior
  change. `impl`/`extend`-block methods are a separate, already-
  documented gap (GENERICS_RULES.md "Known gaps"): neither phase pushes
  a struct's generic scope around them at all, so this fix doesn't
  touch that case either way.

**Tests:** `tests/fixtures/err_param_type_reported_once_isolated.ubl`
and `_combined.ubl` (single-report, both the ownership-wrapper and the
general named-type arity paths); `ok_param_type_single_resolution_
isolated.ubl` and `_combined.ubl` (a generic param stays correctly and
consistently typed across repeated calls with different concrete
instantiations, both for a free function and an inline struct method).
`err_unique_missing_type_argument.ubl`'s header comment updated to
match: it used to document the double-report as expected, pre-existing
behavior; it now expects a single report.

### `sema/lifetime_check.rs`

**What it does:** New pass, well-formedness checking for `[lifetime L]`
/ `[lifetime L where L outlives M]` declarations on functions and
`edge struct`s: LIFETIME-0xx (`UndeclaredLifetime`,
`DuplicateLifetimeParam`, `OutlivesCycle`). Runs right after name
resolution, before type inference, since it's purely structural and
needs neither. See docs/MEMORY_MODEL.md §9 and
docs/DIAGNOSTICS_RULES.md's `LIFETIME-0xx` entry for the full picture;
this is the first of two roadmap slices (well-formedness now, real
outlives/subset enforcement a separate, later, and much larger piece
of work).

**Decisions:**
- Scope picked from three options presented to Abdulhamid (patch the
  raw declaration text only, check declaration well-formedness plus
  every `&name` used in that same declaration's own signature/fields,
  or go straight to real region-inference-level checking): the middle
  one, "well-formedness only," covering both function lifetimes and
  edge-struct lifetimes together rather than sequencing them, per
  direct instruction.
- Deliberately does not walk method bodies or param/return types at
  all. `MethodDecl` has no `lifetime_params` of its own, only an
  enclosing struct can declare any (confirmed by reading the AST, not
  assumed), so checking a method's own `&name` usage would need a
  scope-inheritance story (does a method's `&L` refer to its enclosing
  struct's declared `L`, and if so, which enclosing struct when a
  method reaches this pass outside of `infer_struct_bodies`'s own
  generic-scope push) that this pass doesn't build. Left as a real,
  separate, documented follow-up rather than half-wired.
- `check_type_lifetimes` recurses through the full type structure
  (`List<&L T>`, tuple elements, a function type's own param/return,
  ...), not just a type's top level, so a lifetime name buried inside
  a generic argument still gets checked. Matched against
  `TypeKind`'s full 18-variant surface directly rather than assuming
  which ones could plausibly nest a `Reference`.
- Found empirically, not assumed, while scoping this delivery (see
  docs/MEMORY_MODEL.md §9's own new paragraph on this): marking a
  struct `edge` with a matching `[lifetime L]` currently changes
  nothing about how the existing arena-escape checker (§6) treats it.
  This pass doesn't fix that connection either, `is_edge` still isn't
  consulted by `check_assign_arena_escape` after this delivery, that
  remains real, separate follow-up, but it's why the module doc above
  is explicit that well-formedness checking alone doesn't make `edge
  struct` functional for the case it exists for.

**Tests:** `tests/fixtures/ok_lifetime_wellformed_isolated.ubl` and
`_combined.ubl` (a function with a valid multi-lifetime outlives
constraint, and an edge struct with a matching field, both actually
run, not just type-check); `err_lifetime_undeclared_isolated.ubl` (an
undeclared name in a `where` clause) and
`err_lifetime_cycle_combined.ubl` (the trivial self-outlives case on
an edge struct, next to otherwise-legitimate code, confirming no
cascade).

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
- `eval_expr` has a dedicated arm for `Literal::Int` ahead of the general
  literal arm. A literal whose span is in `Interpreter::int_literal_types`
  becomes the sized `Value` sema picked (`sized_int_from_literal`, plain
  narrowing casts, since sema already range-checked); every other
  literal stays `Value::Int`. A literal directly under unary `-` reaches
  the arm with its positive magnitude and is negated afterwards by
  `wrapping_neg`, which makes `-128` for an `i8` work.
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
- `eval_method_call` (MEMORY_MODEL.md §9's Open Decision #5, method-
  dispatch half): peels off at most one `Unique`/`Shared`/`SyncShared`
  wrapper before its own dispatch match, mirroring `resolve_receiver`
  on the sema side (`builtins/instance.rs`). Checked directly before
  writing this: cloning the unwrapped inner `Value` (the regular
  `Clone`, not `deep_clone`) is O(1) and a mutating method still
  mutates real shared storage regardless, since every collection and
  `Value::Struct` already keeps its own mutable state behind its own
  `Rc<RefCell<...>>`; the clone only shares that inner `Rc`, it never
  duplicates the storage itself.

**Tests:** the four new fixtures under `ok_format_spec_extended_*` and
`ok_format_spec_numeric_base_*` / `err_format_spec_*` exercise all of
this end to end. No unit tests for this function specifically, same
precedent the original precision-only version of this feature already
set (fixture-tested only).

### `builtins/instance.rs`

**What it does:** The canonical registry for every builtin instance
method: which names exist per collection kind, their return shape and
arity, which are HIGH-tier only, and `resolve_receiver`, which strips
tier wrappers off a receiver type before matching it against a kind.

**Decisions:**
- (MEMORY_MODEL.md §9's Open Decision #5, method-dispatch half)
  `resolve_receiver` now also peels off at most one `Unique`/`Shared`/
  `SyncShared` wrapper first, before its existing tier-wrap match.
  Kept as a separate step ahead of the `wrap`/`bare` match rather than
  folded into it: `ReceiverWrap` exists specifically to remember which
  *tier* wrap to reapply to an allocating method's result
  (`method_return_type` in `type_infer.rs`); ownership doesn't need
  that, nothing a builtin instance method returns needs `Unique`/
  `Shared`/`SyncShared` re-applied, only whatever tier the receiver
  already had.
- New `is_builtin_instance_method_name`, a name-only check across all
  nine `ReceiverKind`s' own `METHOD_NAMES`, added specifically for
  `sema/move_facts.rs` to consult (see that module's own entry below);
  it has no type information available to know which kind a given call
  site's receiver actually is, so this is deliberately name-based, not
  kind-specific.

### `sema/move_facts.rs`

**Decisions:**
- (MEMORY_MODEL.md §9's Open Decision #5, method-dispatch half) Once
  `resolve_receiver`/`eval_method_call` made
  `Unique<List<int>>.push(5)` legal, the walker's previous blanket "a
  method-call receiver is a move of its receiver" rule would have made
  the type nearly unusable: every call after the first on the same
  value would have been flagged as a use-after-move. New third opacity
  rule in `MoveExprWalker::visit_expr`: a call whose callee is a known
  builtin instance method name (`instance::is_builtin_instance_method_
  name`) no longer visits its own receiver as a bare use, while still
  walking the receiver normally when it is not a bare identifier (a
  move could be buried deeper inside it) and always walking every arg.
  Name-based, not type-based, same restraint `is_unique_new_call`
  already uses elsewhere in this file. Confirmed empirically, not just
  theorized, that the resulting name-collision gap is a real one, not
  a rare edge case: a hand-built `Counter` struct with its own
  `get(self)` method, wrapped in `Unique`, was silently exempted from
  move-checking too, purely because `get` also happens to be a real
  `List`/`Dictionary`/`Pool` method name. Left as is, real follow-up
  once the language is more mature, direct instruction rather than
  something to silently paper over now.

### `ast/visitor.rs`

- `visit_static_decl` / `walk_static_decl` added for `Item::Static`; the
  walker visits the initializer expression.
- `walk_arg_kind` bumped from private to `pub(crate)` so
  `sema/move_facts.rs` could call the exact same two-line arg-walking
  logic `ast::visitor::walk_expr`'s own `Call` handling already uses,
  rather than a second copy of it living in a different module.

### `lexer/logos_lexer.rs`

**What it does:** The actual token generator, a `logos`-derived enum
(`LogosToken`) mapped onto the public `TokenType` the rest of the
compiler sees.

**Decisions:**
- New `#[token("#")] Hash` variant, modeled directly on the adjacent
  `At` (`@`) token. `#` was not a lexable character in this language
  at all before this delivery. Needed for the alternate-form flag in
  a format spec (docs/PRINT_FORMAT_RULES.md §4).
- (this delivery) `handle_logos_token` and the old `tokenize()` outer
  loop were merged into one new primitive, `next_token`, which
  produces exactly one real token per call (or `None` at true end of
  input) instead of dispatch logic that only ever ran from inside
  `tokenize()`'s own loop. `tokenize()` is now just `next_token()`
  called in a loop until exhausted, plus the trailing `Eof`. The
  actual reason for the split: `lexer/string_parser.rs`'s
  interpolation-hole scanner now drives this exact primitive directly
  on a fresh `LogosLexer` instance, so a hole's closing brace is found
  by tracking real tokens instead of a second, separate raw-byte scan.
  One dispatch path, used both ways, instead of two that could drift
  apart.
- (this delivery) Added `take_lexical_errors`, a `pub(crate)`
  passthrough to the lexer's own `ErrorManager`, so a caller driving
  `next_token()` directly (rather than going through `tokenize()`'s
  `Result`) can still learn whether anything failed.

### `lexer/string_parser.rs`

**What it does:** Parses interpolated (`$"..."`), verbatim (`@"..."`),
and interpolated-verbatim (`$@"..."`) string literals: the outer
text/hole alternation, escape sequences, and each interpolation hole's
own token vector.

**Decisions:**
- (this delivery) `parse_interpolation_expr` (finds a hole's closing
  `}`) no longer counts raw `{`/`}` bytes to find the boundary. It
  drives a fresh `LogosLexer` over the rest of the file one token at a
  time (via the new `next_token` primitive, see `lexer/logos_lexer.rs`
  above) and tracks depth using genuine `LeftBrace`/`RightBrace`
  TOKENS. Anything already living inside a nested string, char
  literal, or comment is consumed atomically by the same dispatch used
  everywhere else, including recursively for a nested `$"..."`, so it
  can never be mistaken for a hole boundary the way a raw byte scan
  would. See "Fixes and Problems" below for the bug this replaces.
- (this delivery) A hole's own `Vec<Token>` still ends with a synthetic
  `Eof`, matching what the old two-phase approach (calling `tokenize()`
  on an isolated substring) produced. This isn't cosmetic:
  `rd_parser::Cursor::peek_token` clamps its index access on the
  explicit assumption that every token slice handed to it ends with
  `Eof` (`tokens[pos.min(len - 1)]`), so a hole's tokens without one
  would misbehave the moment a real `Parser` consumed them (see
  `crates/rd_parser/src/parsers/parse_expr.rs`'s `parse_interp`), even
  though nothing in this crate's own tests would catch it.

### `lexer/token.rs`

**What it does:** The public `TokenType` enum every other crate sees,
plus its `Display` impl (used for "expected X, found Y" parser error
messages).

**Decisions:**
- `Static` added next to `Const` in the enum and the `Display` impl, and
  to the `KEYWORDS` phf map and `LogosToken` (`logos_lexer.rs`). `static`
  is now a reserved word; no existing fixture or example used it as an
  identifier.
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
- New `int_literal_types: HashMap<Span, IntSuffix>` field and
  `set_int_literal_types` setter. The interpreter has no static types, so
  sema's `SemaContext::int_literal_types` is the only way it learns that
  the `5` in `let x: u8 = 5` is a `u8`. It defaults to empty, which keeps
  every literal a plain `int`, the behavior of the interpreter's own unit
  tests that skip sema. Any driver that runs a program after sema must
  call the setter before `run_program`: `ubel run`, the `pipeline` and
  `diagnose` examples, and the wasm playground do.
- `eval/pattern.rs` takes `PatternTables { enums, aliases }` (a `Copy`
  pair of references) instead of a bare enum table, so a struct pattern or
  an enum path pattern written with an alias (`P { x, y }`, `C.Red`) is
  compared against the real type name through `canonical_type`.
- `match_literal` (`eval/pattern.rs`) compares an unsuffixed integer
  literal pattern against every sized-integer variant through `i128`, so
  `match byte { 255 => .. }` works on a `u8`.
- Traits: `trait_names`, `trait_method_table` (`(type, trait)` to method to
  function) and `trait_call_sites` (callee span to trait, from sema, set with
  `set_trait_call_sites`). `register_trait_impl_methods` registers an impl's
  methods and the trait's defaults it does not override into both the
  per-trait table and, with `or_insert`, the flat `method_table`, so an
  inherent method wins in either order. A call in `trait_call_sites`, and a
  written `Trait.m(value)`, dispatch through the per-trait table. Aliases and
  trait defaults are collected before the main registration loop, so
  declaration order does not matter, and `extend P` on an alias extends the
  real struct.
- New `type_aliases: HashMap<String, String>` (alias name to the struct or
  enum name it stands for; only aliases whose target is a named type) and
  `canonical_type`, which follows it with a hop limit (sema rejects a
  cycle; the limit only guards sema-less unit tests). It is applied where a
  value is BUILT or a name is DISPATCHED on (a struct literal, an enum
  variant of every payload shape, a static call, a method-table lookup), so
  values carry the real type name and a method written on `Point` finds a
  value built as `P { .. }`. It returns the name unchanged without
  allocating when the table is empty.
- `eval_method_call` dispatches a `Value::Enum` through the same
  `method_table` as a struct (no derives, so no `.clone()` pseudo-method),
  and when a struct has no method of the called name but has a field of
  that name holding a function, calls the field. A method wins over a
  field, matching sema.
- New `statics: HashMap<String, Value>` field for `static` items. It is
  deliberately NOT part of `env`: `call_function` replaces `self.env` with a
  clone of the function's closure for every call (and `Environment::
  snapshot` is a plain clone), so a global kept in a scope is copied in,
  assigned to, and discarded when the call returns, and no other function
  ever sees the change. `lookup` consults `env` first and then `statics`;
  `write_lvalue` (`eval/expr.rs`) does `env.set`, then the static slot, and
  defines a new local only if the name is neither. That order is what makes
  a local or a parameter shadow a static. Heap values are shared by
  reference as elsewhere, so `STATE.hits += 1` and `LIST.push(x)` need no
  special handling, and a lambda needs none either since it never captured
  the static in the first place.
- The startup retry loop that evaluates constants now carries
  `(name, value expr, is_static)` triples, so a static can refer to a
  constant or to another static declared later in the file, and a cycle is
  reported as `initializing static`/`constant` with the name. Constants
  still go to `env` (and are backfilled into every closure); statics go to
  `statics`, which needs no backfill. The `ConstDecl` import is gone since
  the loop no longer holds one.
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

### `error_management/errors/naming/mod.rs`, `tier/mod.rs`, `parse/mod.rs`

**What it does:** `NameError` (NAME-0xx), `TierError` (TIER-0xx) and
`ParseError`/`ParseContext` (PARSE-0xx).

**Decisions (`static` items):**
- `NameError::StaticInConst` (`NAME-008`): an identifier inside a `const`
  initializer resolved to a `static`.
- `TierError::StaticAccessOutsideHigh` (`TIER-015`): a `static` was read or
  written from a `@tier(mid)` or `@tier(low)` function. TIER-007 stays
  retired; 015 is the next free code after 014.
- No new `ParseError` variant. A tier annotation on a `const` or a wrong
  one on a `static` reuses `IllegalInContext` (`PARSE-004`), which already
  exists for "valid token, illegal here". `ParseContext::StaticDecl`
  ("static declaration") was added for `PARSE-001` raised inside
  `parse_static_decl`.

### `error_management/errors/types/mod.rs`

**What it does:** `TypeError`, errors from ordinary type inference and
type checking (TYPE-1xx range).

**Decisions:**
- `TypeError::NotATrait` (`TYPE-122`), `TraitMethodMissing` (`TYPE-123`),
  `UnknownTraitMethod` (`TYPE-124`), `TraitMethodSignatureMismatch`
  (`TYPE-125`), `UnsatisfiedBound` (`TYPE-126`), `AmbiguousTraitMethod`
  (`TYPE-127`), `OverlappingImpl` (`TYPE-128`), `UnsupportedTraitFeature`
  (`TYPE-129`) and `MethodOnUnboundedParam` (`TYPE-130`): see
  `docs/DIAGNOSTICS_RULES.md`.
- `TypeError::TypeAliasCycle` (`TYPE-121`): a `type` alias that refers to
  itself, directly or through others.
- `TypeError::IntLiteralOutOfRange` (TYPE-120) gained `negative` and
  `inferred`: `raw` is always the magnitude, `negative` says the literal
  sat directly under unary `-`, and `inferred` marks an unsuffixed
  literal whose context picked the type (the message then does not
  repeat a suffix after the digits).
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

### `sema/name_resolution.rs` (assignment to a constant)

- Nothing stopped `LIMIT = 6` on a global `const`. Added
  `NameError::AssignToConst` (`NAME-007`). Checked against the
  scope-resolved definition's kind rather than the bare name, so a local
  `let` that shadows a constant's name stays assignable.

### `sema/type_infer.rs`

- A `type` alias became a nominal `Named` type of its own, so `let a: Score
  = 5` with `type Score = int` was `TYPE-101` and `Score` and `int` did not
  unify in either direction (generic aliases were broken the same way).
  Aliases are now expanded to their targets, see "Decisions (type
  aliases, ...)" above and `docs/PARKED_IDEAS.md`, "Taking down the
  confirmed bugs".
- An unknown method on an enum value fell through unchecked unless the
  enum had an `extend` block, and a method declared in an `extend` block
  on an enum passed sema but panicked in the interpreter, which dispatched
  user methods on structs only. Both fixed together, so the two sides agree.
- Calling a function-typed struct field (`c.cb(4)`) was `NoSuchMethod`
  because the `Call` arm only consulted `struct_methods` for a `Field`
  callee. A field of that name is now called, after a method of that name.
- `structurally_compatible` invented `Span::at(0)` for every nested
  mismatch (six sites), so a diagnostic about `List<int>` against
  `List<string>` pointed at the top of the file.
- `Color.nothing()` reported `UnknownVariant` twice, once from the callee
  `Field` arm and once from the `Call` arm.
- `binop_result`'s orderable list covered `int`, `float`, `double` and
  `string` only, so `a < b` on two `u32` (or `i64`, `u8`, `f32`, and so
  on) was `TYPE-118` even though the interpreter's sized-integer
  comparison handles every ordering operator. The sized integers and
  `f32`/`f64` are now in the list.
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
- `Dictionary<K, V>` was missing from `structurally_compatible`, so an
  annotated dictionary failed against `Dictionary<?T, ?T>`. Key and
  value arguments are now unified. Same bug class as `Set`/`Queue`/
  `Stack`.
- An unknown method on a struct reported both `NoSuchField` and
  `NoSuchMethod`. The `Call` arm sets `callee_field_pending` before
  pre-inferring a `Field` callee and the `Field` arm takes it first
  thing, so the flag applies to that node only.
- (MEMORY_MODEL.md §9, Open Decision #5, user-struct half) The
  struct-instance-method branch of the call arm only matched a bare
  `SemaType::Named` receiver, so `Unique<T>`/`Shared<T>`/`SyncShared<T>`
  wrapping a user struct fell through to `NoSuchMethod` even though the
  method existed. `instance::resolve_receiver` already peels these
  wrappers but returns `None` for a user struct, discarding the peeled
  type, so it could not be reused. Fixed by peeling one wrapper locally
  before the `Named` check; the wrapper is deliberately not reapplied
  to the return type, matching `resolve_receiver`'s own documented
  decision. Sema only: `eval_method_call` already peeled all three for
  every receiver.
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

- Global `const` items were name-resolved and type-checked but never
  evaluated, so any read from a function body panicked with `undefined
  name`. `run_program` now evaluates them after the first closure
  backfill, so an initializer can call a top-level function, retrying
  constants that hit an undefined name until a pass makes no progress
  (sema accepts a constant that refers to one declared later in the
  file), then backfills every function's closure a second time so all of
  them see the constants. A constant cycle is reported at startup.

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

### `lexer/string_parser.rs`

- `parse_interpolation_expr` found a hole's closing `}` by counting raw
  `{`/`}` bytes with no awareness of strings, char literals, or
  comments. This happened to come out right whenever whatever was
  nested inside had internally balanced braces (a plain nested string,
  even a nested `$"..."`), which is why the language appeared to
  already "support" nested strings in holes. It broke specifically when
  something nested contained a genuinely unbalanced brace character,
  which is ordinary: a string like `"missing an { arg"`, a comment
  mentioning a brace, a char literal `'{'`. Confirmed empirically with a
  throwaway probe against the real lexer before choosing a fix, not
  assumed from reading the code; the doc note this replaces
  (docs/PRINT_FORMAT_RULES.md's old §8) had slightly mischaracterized
  the trigger as "any nested string", not specifically an unbalanced
  one. Three fix options were presented to Abdulhamid: patch the raw
  scanner to be string/comment-aware by hand, drive the real lexer
  incrementally, or bulk-tokenize-and-walk. The incremental,
  token-driven option was chosen for architectural correctness (no
  duplicated string/comment logic to drift out of sync later) over the
  smaller diff either of the other two would have been. See
  `lexer/logos_lexer.rs`'s `next_token` above for the primitive this is
  built on.
- Caught during this delivery's own fixture sweep, not before: the
  first pass at the fix dropped each hole's trailing `Eof` token, since
  the new `next_token` primitive deliberately never produces one (only
  `tokenize()`'s own wrapper does). `rd_parser::Cursor` depends on that
  `Eof` being present (see the Decisions note above); every relocated
  `string_interpolation_test.rs` assertion that checked a hole's exact
  token list failed with an extra-token mismatch until it was added
  back explicitly.

### `lexer/logos_lexer.rs`

- `handle_error`'s span used `span_range.start`/`span_range.end`
  directly instead of the `abs_start`-based computation the ordinary-
  token path already used. `span_range` comes from
  `self.logos_lex.span()`, which is relative to whichever slice
  `self.logos_lex` currently wraps, not to the whole file, once at
  least one string or comment earlier in the file has caused a rebase
  (`self.logos_lex = LogosToken::lexer(&self.input[pos..])`). This is
  the exact same bug already found and fixed for the normal-token path
  (see that fix's own comment, still in the code); it had just never
  been mirrored into the "unexpected character" error path. Found and
  fixed while restructuring this function for the `next_token` split
  above, not sought out separately.

### `crates/rd_parser/examples/pipeline.rs`

- The `[SEMA-FAIL]` reporting block explicitly enumerates
  `take_name_errors`/`take_type_errors`/`take_tier_errors`/
  `take_borrow_errors`/`take_move_errors` one by one, rather than
  walking every error category generically. Adding a new category
  (`LifetimeError`) meant this script, used by both local fixture
  sweeps and `ci-check.yml`, silently printed `[SEMA-FAIL]` with no
  detail at all for any file that failed only on a lifetime error,
  found while empirically verifying `sema/lifetime_check.rs` against
  real probes, where every one of them showed a blank error list. Added
  the missing `take_lifetime_errors` loop, same shape as its siblings.
  Same class of gap `DIAGNOSTICS_RULES.md` §9's own case study already
  names as the thing that *can* still drift silently even with the
  registry discipline: a new error class not being drained by every
  place that walks `ErrorManager`'s output.

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

A third, separate delivery this session (the nested-string-literal-
inside-interpolation-hole fix, roadmap housekeeping item 3(c)) touched
`lexer/string_parser.rs` and `lexer/logos_lexer.rs` again, plus
`docs/PRINT_FORMAT_RULES.md` (removing its now-resolved §8 entry).
`lexer/string_parser.rs` did not have a NOTICE header before this
delivery; one was added, matching the convention `lexer/logos_lexer.rs`
already used. Same discipline followed as the note above: no em dashes
or first/second person in anything actually added or rewritten, not a
retroactive sweep of either file's pre-existing comments.

A fourth delivery this session (Open Decision #6, docs/MEMORY_MODEL.md
§12, the last housekeeping item on the roadmap before design-only work
on outlives scoping) touched `sema/type_infer.rs` and
`sema/sema_context.rs`, plus `docs/MEMORY_MODEL.md` §12's own status
row. Same discipline again: only the lines actually written this
delivery were checked for em dashes and first/second person, not a
sweep of `type_infer.rs`'s substantial pre-existing prose elsewhere in
the same file.

A fifth delivery this session (roadmap item 4, first slice: well-
formedness checking for `[lifetime L]`/`[lifetime L where L outlives
M]`, scoped as design-only in a prior session and picked up for real
implementation after presenting depth options and getting "well-
formedness only, both areas" back) added a new file,
`sema/lifetime_check.rs`, a new error family (`error_management/
errors/lifetime/mod.rs`, `LIFETIME-0xx`), touched `sema/mod.rs` and
`error_management/error_manager.rs` to wire the new pass in, touched
`crates/rd_parser/examples/pipeline.rs` for the reporting-gap fix
above, and touched `docs/MEMORY_MODEL.md` §9 and
`docs/DIAGNOSTICS_RULES.md`'s registry. Same discipline again: checked
every line this delivery actually wrote, not a sweep of any of these
files' substantial pre-existing content.

A sixth delivery this session (MEMORY_MODEL.md §9's Open Decision #5,
method-dispatch half: does `resolve_receiver` strip `Unique`/`Shared`/
`SyncShared` to dispatch methods on the inner type) touched
`builtins/instance.rs`, `interpreter/eval/expr.rs`,
`sema/move_facts.rs`, `ast/visitor.rs` (one line, a visibility bump),
and `docs/MEMORY_MODEL.md` §9's own Open Decision #5 entry plus its
"one deliberate over-approximation" paragraph just above it, updated
rather than left stale once the over-approximation it described no
longer matched the code. Same discipline again: checked every line
this delivery actually wrote or rewrote, not a sweep of any of these
files' substantial pre-existing content.

A seventh delivery (roadmap item 7, user-struct half: method dispatch
through `Unique`/`Shared`/`SyncShared` for user-defined `extend`/`impl`
methods) touched `sema/type_infer.rs` (one branch), the module doc of
`sema/move_facts.rs` (stale, corrected to describe the by-name exemption
and the remaining user-method gap), `docs/MEMORY_MODEL.md` §9 (a stale
paragraph and Open Decision #5's row), `docs/PARKED_IDEAS.md`,
`docs/OUTLIVES_RULES.md` (header still read "Design, Not Yet Built"
after all four phases landed), and the public pages
`web/site/src/project-status.md` and `web/site/src/tier-model.md`,
which had drifted behind the outlives, `extend`/`impl` dispatch, and
sized-integer work. Same discipline again: checked every line this
delivery actually wrote or rewrote, not a sweep of pre-existing content.

An eighth delivery (the queue from the second external test session:
global `const` evaluation and `NAME-007`, the move-check exemption for
user-declared methods, nested generic `>>`, the lambda return type
diagnostic, `Dictionary` unification, and the duplicate `TYPE-103`/
`TYPE-104`) touched `sema/name_resolution.rs`, `sema/type_infer.rs`,
`sema/move_facts.rs`, `sema/move_check.rs`, `interpreter/eval/mod.rs`,
the parser's `cursor.rs`, `parse_type.rs`, `parse_expr.rs` and
`parse_stmt.rs`, plus `PARSER_RULES.md` §5.9, `DIAGNOSTICS_RULES.md`,
`GENERICS_RULES.md`, `TESTING_RULES.md`, `PARKED_IDEAS.md`,
`MEMORY_MODEL.md`, `ubel.ebnf`, both crate docs, and the public
`project-status.md`. It also corrected two statements the previous
delivery had written (the `move_facts.rs` module doc and a
`MEMORY_MODEL.md` sentence) that this delivery made stale. Same
discipline: checked every line this delivery wrote or rewrote.

A ninth delivery (the two decisions recorded as "Decided, not yet built"
after the tooling stubs: context-driven integer literal typing, then
mutable globals as `static` items) touched `sema/type_infer.rs`,
`sema/sema_context.rs`, `sema/name_resolution.rs`, `sema/tier_check.rs`,
`sema/symbol_table.rs`, the interpreter's `eval/mod.rs`, `eval/expr.rs` and
`eval/pattern.rs`, the lexer, the AST, the visitor, three error modules,
the parser's `parse_decl.rs` and sync sets, the `ubel` CLI, the examples
and wasm playground, plus `PARSER_RULES.md` section 5.7a,
`DIAGNOSTICS_RULES.md`, `TESTING_RULES.md`, `PARKED_IDEAS.md`,
`ubel.ebnf`, both crate docs and the public `project-status.md`. The
literal typing half shipped first as its own bundle; the `static` half is
cumulative on top of it. Same discipline: checked every line this delivery
wrote or rewrote for em dashes and first or second person.

A tenth delivery (taking down the four confirmed bugs, then the traits
design session next) touched `sema/type_infer.rs`, the interpreter's
`eval/mod.rs`, `eval/expr.rs` and `eval/pattern.rs`, the TypeError enum,
and added three integration test files (`type_aliases.rs`,
`member_calls.rs`, `diagnostic_spans.rs`) and twelve fixtures. It is
cumulative on top of the static-globals delivery. Every reproduction was
run and its result seen before the fix, and each test file was
mutation-checked: reverting a fix makes exactly the tests that guard it
fail. Same discipline: every line this delivery wrote or rewrote was
checked for em dashes and first or second person.

An eleventh delivery (traits slice S1a) touched `sema/type_infer.rs`,
`sema/sema_context.rs`, the interpreter's `eval/mod.rs` and `eval/expr.rs`,
the TypeError enum, the `ubel` CLI, both examples and the wasm playground
(each now passes `trait_call_sites`), and added `tests/traits.rs` and sixteen
fixtures. It is cumulative on the confirmed-bugs delivery. Same discipline:
every line it wrote was checked for em dashes and first or second person.

**Decisions (traits, slice S2a part 1):**
- `ubel_stratum_rd::prelude` holds the prelude as Ubel source (`Ordering`,
  `PartialEq`, `Eq`, `PartialOrd`, `Ord`, `Clone`). `parse()` calls
  `prelude::inject`, which parses it with every token span shifted past
  `PRELUDE_SPAN_START` (`Span::is_prelude`) and puts its items in front of the
  program's, leaving out any the program declares itself and everything that
  depends on it (`DEPENDS_ON`).
- Sema records the prelude traits when `collect_trait_info` sees a prelude span
  (`prelude_traits`, `prelude_defs`). `prelude_status` answers whether a type
  has one, from the table in `docs/TRAITS_DESIGN.md`; `bound_status` and
  `type_implements` use it for a prelude trait instead of `trait_impls`.
  `PRELUDE_TRAITS` carries the supertraits, and `expand_supertraits` widens the
  bounds in force inside a body (declared bounds are not widened).
- `try_prelude_method` resolves `x.eq(y)`, `x.cmp(y)` and the rest on a value
  that is not a type parameter, by calling `call_through_bounds` with the
  trait as the only bound. It runs before the user-type paths and inside the
  built-in-kind path, and yields to a method the type declares itself.
- `check_operator_bound` raises `TYPE-131` for `==`, `!=` and the ordering
  operators on a type-parameter operand without the trait.
- A hand-written impl of a prelude trait is `TYPE-129` in
  `register_trait_impls` and is skipped. The interpreter runs a prelude trait's
  method natively (`native_prelude_method` in `eval/expr.rs`, reached from the
  trait call site table and from the qualified call) through `eval_binop`.

A twelfth delivery (traits slice S1b) touched `sema/type_infer.rs` and the
TypeError enum (`TYPE-130`), and added `tests/bounds.rs` (46 tests) and ten
fixtures. It is cumulative on the S1a delivery and needs no change to the
interpreter, the CLI, the examples or the wasm playground. The fixture sweep
went from 212 lexed, 204 parsed, 110 through sema, interpreter and full
pipeline to 222, 214, 114, 114 and 114, and the only changes were the ten new
files: no existing fixture changed its result. The test file was
mutation-checked: each of seven reverted fixes fails the tests that guard it
and no others of the file, and the one mutation that first failed nothing (the
span sort) led to an added test. Same discipline: every line it wrote was
checked for em dashes and first or second person.

A thirteenth delivery (traits slice S2a, part 1) touched `rd_parser/lib.rs`
(`parse()` injects the prelude), `lexer/token.rs` (the reserved span range),
`sema/type_infer.rs`, `interpreter/eval/expr.rs` and the TypeError enum
(`TYPE-131`), and added `rd_parser/src/prelude.rs`, `tests/prelude_traits.rs`
(40 tests), five unit tests in `prelude.rs` and nine fixtures. It is
cumulative on the S1b delivery, includes the S2 design section of
`docs/TRAITS_DESIGN.md`, and changed one S1b test and one S1b fixture that
had assumed `T: Ord` was `TYPE-129`. The fixture sweep went from 222 lexed,
214 parsed, 114 through sema, interpreter and full pipeline to 231, 223, 119,
119 and 119, and the only changes were the nine new files. Each of ten
reverted fixes fails the tests that guard it; one mutation (supertrait
expansion) first failed nothing because it reverted only one of two paths, and
was redone on the function itself. Same discipline: every line it wrote was
checked for em dashes and first or second person.
