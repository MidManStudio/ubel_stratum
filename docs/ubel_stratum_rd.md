# ubel_stratum_rd

## Overview

Recursive descent parser crate for Ubel Stratum. Lexes and parses
source text into the AST types defined in `ubel_stratum`. See
`PARSER_RULES.md` for the architecture (recursive descent for
declarations and statements, Pratt parsing for expressions, targeted
memoization for the few genuinely ambiguous cases).

## Modules

### `parsers/parse_pattern.rs`

**What it does:** Parses match arm patterns and struct/array destructure
patterns.

**Tests:** see `tests/fixtures/ok_wildcard_and_discard_isolated.ubl`,
`tests/fixtures/ok_callback_registry_combined.ubl`, and
`tests/fixtures/err_underscore_in_expression_position.ubl`.

### `parsers/parse_decl.rs`

**What it does:** Parses top-level and member declarations: functions,
methods, parameters, structs, enums, traits, impls.

**Decisions:**
- Added a `TokenType::Underscore` arm to `parse_param` for `_: Type`
  parameters, producing `ParamKind::Discard`.
- `parse_static_decl` parses `[pub] static NAME: Type = expr` into
  `Item::Static`. The type annotation is required, and every failure
  path emits its diagnostic and then calls `recover_to_decl()` so the
  rest of the declaration is not parsed a second time as a stray item.
  `apply_block_attrs` merges only the generic attributes into a static,
  never a tier. See `docs/PARSER_RULES.md`, section 5.7a.
- `parse_item_or_block` checks the item's OWN `@tier(...)` attribute
  (found by name in the attributes parsed directly in front of it, not the
  block-merged list): on a `const` it is `PARSE-004`; on a `static`,
  `@tier(mid)` and `@tier(low)` are `PARSE-004` and `@tier(high)` is
  accepted. A tier block around either is not reported.
- `TokenType::Static` joined the three declaration sync sets.
- `parse_trait_decl` reads an optional supertrait list through
  `parse_supertraits`, and a type parameter's bounds and the supertrait list both
  go through `parse_trait_bound_list` (in `parse_type.rs`): `A + B<int>` after
  the name and any generic parameters, or after a parameter's `:`, stored as
  `TraitBound { name, args, span }` per entry. A `<` after a bound's name always
  opens its arguments, which are read with `try_parse_generic_args`. A colon or a
  `+` with no name after it is one parse error (`expected supertrait name`, or
  `expected trait bound name`) and the parse goes on. The list is sized with
  `cap::BOUNDS`.
- `parse_generic_params` closes the list with `at_generic_close` and
  `eat_generic_close`, not a plain `>`: a bound with arguments that ends the list
  (`T: Convert<int>>`) closes with one `>>` token, which is two closes.
- `parse_impl_block` keeps the type arguments written after the trait when `for`
  follows (`impl Convert<int> for X`, stored as `trait_args`) and drops them for an
  inherent impl (`impl Box<int> { .. }`), where they are part of the type.
- The tier of a method is resolved here, in this order: its own `@tier`, the `@tier` of
  the `impl`, `extend` or struct around it, a tier block around that, then `HIGH`
  (`PARSER_RULES.md` 5.11). `has_own_tier` in `parse_attr.rs` decides by the attribute's
  name whether a method writes a tier, so an unrelated attribute such as `@inline` keeps
  the block's tier. `parse_struct_decl` hands the struct's own `@tier` to
  `parse_struct_member`, `parse_trait_decl` hands the trait's to `parse_trait_item` (which
  gives a signature `MethodSig.tier` and a default method `MethodDecl.tier`), and
  `apply_block_attrs` moves the methods of a struct, an `impl`, an `extend` or a trait that
  write none to the block's tier (`retier_methods`, `retier_members`, `retier_trait_items`).
  `MethodSig` gained `attributes` and `tier`; nothing else was added to the AST. An impl
  method that writes no tier is decided in sema (`PARSER_RULES.md` 5.11).

**Tests:** see `tests/fixtures/ok_wildcard_and_discard_isolated.ubl` and
`tests/fixtures/ok_callback_registry_combined.ubl`; for `static`, the
five `ok_static_*` and seven `err_*static*`/`err_const_tier_*` fixtures
and `tests/statics.rs` below.

### `tests/type_aliases.rs`, `tests/member_calls.rs`, `tests/diagnostic_spans.rs`

**What it does:** Integration tests for the four confirmed bugs taken
down together (`docs/PARKED_IDEAS.md`, "Taking down the confirmed bugs").
Each goes through `check_source` for the stage and codes, and through the
interpreter (sema, then `set_int_literal_types`, then `run_program`) where
a test needs runtime values.

**Decisions:**
- `type_aliases.rs` (20): transparency in type position for every alias
  shape, `TYPE-121` once per alias in a cycle and not for an unrelated
  alias beside it, no cascade from a cyclic alias, `TYPE-108` for a wrong
  generic count, and the expression and pattern positions (struct literal,
  associated call, every enum variant shape, struct and enum patterns), with
  values asserted to carry the real type name.
- `member_calls.rs` (17): an unknown method on a plain enum and on an enum
  with an `extend` block, one `UnknownVariant` per typo (not two), enum
  methods running on unit and payload variants, associated functions on an
  enum, and function-typed fields (called, closures, generic holders, a
  method winning over a field, wrong argument type, wrong return type, a
  non-function field, an unknown name).
- `diagnostic_spans.rs` (3): asserts the LINE of a nested generic mismatch,
  which a count-only test cannot see.
- Mutation-checked: reverting the span fix fails exactly the three span
  tests; removing enum dispatch fails exactly the four enum-runtime tests;
  making aliases nominal again fails 15 of the 20 alias tests (the other 5
  exercise the separate `type_def_of` construction path).

### `tests/traits.rs`

**What it does:** Integration tests for nominal traits, slice S1a (42
tests). Source goes through `check_source` for the stage and codes, and
through the interpreter (sema, then both setters, then `run_program`) where a
test needs runtime values.

**Decisions:**
- Pins exact codes and counts for `TYPE-122` to `TYPE-129`, the exact text
  of the signature-mismatch and ambiguity messages, declaration-order
  independence, inherent-before-trait in both block orders, and the runtime
  contract that a call made through a trait runs the trait's method. One
  test runs without `set_trait_call_sites` to document why that table exists.
- Mutation-checked: removing the call-site recording fails the one test that
  expects the trait's method (18, not 14); removing the signature comparison
  fails the three `TYPE-125` tests; the original inherent-versus-two-traits
  bug fails two tests.

### `tests/supertraits.rs`

**What it does:** Integration tests for supertraits, slice S3a (65 tests).
Source goes through `check_source` for the stage, codes, messages, suggestions
and positions, through the parser for the recorded shape and spans, and through
the interpreter (sema, then both setters, then `run_program`) where a test needs
runtime values.

**Decisions:**
- Pins the parsed shape (none, one, several in the written order, after generic
  parameters, on a bodiless or public trait, the span of each name, and one
  parse error for a colon or a `+` with no name), the prelude's own declared
  supertraits, the exact text and count of `TYPE-132` (first missing link,
  transitive links, source order, derive versus impl suggestion), what a bound
  implies in a body and at a call (and that the implication runs one way), the
  default body's view of its supertraits, names (`TYPE-122`), cycles (`TYPE-133`:
  direct, two, three, two separate, a trait that only leads into one, a diamond,
  a repeated name, the path text, the span, the suggestion), shadowing, and the
  runtime contract that a call through a bound runs the supertrait's method and
  not an inherent method of the same name.
- Four fixtures sit beside it: `ok_trait_supertrait_isolated`,
  `ok_trait_supertrait_combined`, `err_trait_supertrait_missing_impl_isolated`
  and `err_trait_supertrait_names_combined`.
- Mutation-checked: 34 mutations of the parser, the prelude, the supertrait
  registry, the cycle walk, the closure, the impl check and the error enum; 32
  are caught. The first pass left three alive. One exposed a weak test (a
  repeated supertrait only matters in a self-cycle) and is now killed; the cycle
  walk's visited check is a performance guard that changes no diagnostic; and the
  `Self` bound that `collect_trait_info` inserts is never consulted there
  (removing it entirely changes nothing), so that line was left as it was.

### `tests/generic_traits.rs`

**What it does:** Integration tests for generic traits, bounds with arguments and
generic supertraits, slice S3b (79 tests). Source goes through `check_source` for
the stage, codes, messages, suggestions and positions, through the parser for the
recorded shape, and through the interpreter where a test needs runtime values.

**Decisions:**
- Pins the parsed shape (an impl's arguments, none for an inherent impl, nested
  arguments and the `>>` that closes a bound and the parameter list at once,
  several bounds and parameters in order, a bound naming another parameter, a
  supertrait's arguments), declaring and implementing a generic trait (two
  parameters, different arguments for different types, the argument count at an
  impl, a method that must fit the arguments, a second impl as `TYPE-128`
  whatever its arguments, a trait parameter's own bound), calling through a
  concrete type (the impl's types, defaults, a method taking the parameter, the
  qualified form), bounds with arguments (concrete, naming another parameter,
  decided by the impl, a mismatch after the impl decided, wrong counts including
  inside a trait's own scope, conflicts as `TYPE-134`, struct parameters, aliases,
  a function declared before its trait), generic supertraits (the argument carried
  through one and two levels, a fixed argument, `Self` in an argument, a bound on
  the sub-trait meeting a requirement on the supertrait only with the same
  argument, `TYPE-132` naming the argument, cycles), the prelude traits taking no
  arguments, and the runtime contract that each type runs its own impl, through a
  bound whose argument is a parameter too.
- Four fixtures sit beside it: `ok_trait_generic_isolated`,
  `ok_trait_generic_combined`, `err_trait_generic_impls_isolated` and
  `err_trait_generic_bounds_combined`.
- The old test asserting that a generic trait is reported (`TYPE-129`) became one
  asserting it is accepted.
- Mutation-checked: 41 mutations of the parser, bound registration, the argument
  conversion pass, the supertrait closure, obligations, impl registration, the
  qualified call and the error enum. 39 are caught. One pointed at a dedupe the
  closure already does and was removed as dead code, and the last, the `Self` bound
  that `collect_trait_info` inserts, is never read there (removing the line
  entirely changes nothing), so it is left as it was. The first pass left five
  alive; three needed tests (a bound with too few or too many arguments only
  reaches the scopes built from a trait's own parameters; a conflicting bound
  must not also fail every call).

### `tests/method_generics.rs`

**What it does:** Integration tests for a method's own generic parameters and for
bounds on a type alias parameter, slice S3c (66 tests). Source goes through
`check_source` for the stage, codes, messages and positions, and through the
interpreter where a test needs runtime values.

**Decisions:**
- Alias bounds: declared without error, met and unmet at a use (one `TYPE-126`, at
  the use, once for an annotation), a bound with a trait argument, a name that is
  not a trait, a wrong argument count, conflicting bounds, the same bound twice,
  a use through another alias, a use inside a generic function with and without
  the bound, a two-parameter alias where only one is bounded, an unbounded alias of
  a bounded struct, and the value behaving as the aliased type at run time.
- Own parameters: each call chooses its own types, a wrong result type, a wrong
  argument count, a bound met and unmet (reported at the argument for that
  parameter, once for two arguments of the same parameter), a bound with a trait
  argument, a bound argument the impl decides, a bound naming an earlier own
  parameter, an associated function, `extend` and inherent `impl` methods, a
  generic struct (the own parameter after the struct's, a bounded one asked at its
  own index, a receiver with too few or too many type arguments not shifting it),
  a method taking a function, bounds that build on a supertrait, the body
  (`TYPE-101` naming the parameter, `TYPE-130`, `TYPE-131`, an operator bound), and
  the checks at the declaration (`TYPE-122`, `TYPE-108`, `TYPE-134`, a repeated
  bound, a bad name beside a real bound reported once per declaration).
- Generic methods in traits: an impl that repeats the method (with renamed
  parameters) and runs, one that drops the parameters, drops a bound, adds a bound,
  adds a parameter or has other trait arguments in a bound (`TYPE-125` with the
  exact text), the order of bounds not mattering, a generic trait with a generic
  method, a bound that names the trait's parameter, `Self` beside an own parameter,
  a default body with its own bounds, calls through a bounded parameter and in the
  qualified form (types chosen per call, bound checked), an enum implementor, and
  an impl body checked against its own parameters.
- Six fixtures sit beside it: `ok_alias_bound_isolated`, `err_alias_bound_isolated`,
  `ok_method_generic_isolated`, `err_method_generic_isolated`,
  `ok_method_generic_combined` and `err_method_generic_combined`. The two tests in
  `tests/bounds.rs` that pinned the two limits as `TYPE-129` now assert that a
  bound there is declared without error. `err_bound_unsupported_combined` no longer
  has anything to show and is removed by the patch that comes with the delivery.
- Mutation-checked: 45 mutations of the alias bound registration and use, the own
  parameter scope, the bound checks, the instantiation per call, the renumbering of
  a trait method's parameters, the impl comparison, the signature text, the call
  sites and the display of parameter names. The first pass left nine alive and one
  did not build. Six needed tests (a bad name beside a real bound must not be
  reported twice; a receiver with too few or too many type arguments must not shift
  the method's own parameters; a bounded own parameter on a generic struct is asked
  at its own index; a bound that names the trait's parameter; a signature that names
  `Self` beside an own parameter). Three changed nothing a program can observe and
  were removed: an early `continue` for a parameter without bounds, and two guards
  in `instantiate_trait_shape`, whose argument list always has the trait's arity
  (now a debug assertion). All 42 that remain are caught.

### `tests/generic_impls.rs`

**What it does:** Integration tests for impls and `extend` blocks for generic types,
slice S3e (82 tests), plus the extended unit test in `interpreter/value.rs` that pins
`Value::impl_key` against `ast::types::builtin_impl_key` for the five generic
collections. Programs go through `check_source` for stage, codes, messages,
suggestions and positions and through the interpreter for run-time values.

**Decisions:**
- Parameters and scope: a parameter bound in the bodies, numbered by the place it
  occupies (`Pair<Y, X>`), an enum head, an inherent `impl<T>`, `extend<T>`, a
  method's own parameters inside a generic block, an associated function, `Self`.
- Conditions: a conditional impl through a bound, a qualified call and a method
  call; the argument that lacks the bound named at the call; a struct's own bound
  restated, checked on a concrete argument and in force in the body; a bound with
  arguments naming another parameter; a dictionary key needing `Hash`.
- Supertraits and prelude traits on a generic impl, including a conditional
  `PartialEq` and `PartialOrd` driving the operators and not there for an argument
  that lacks them, and a derive that overlaps a hand-written impl.
- Concrete positions: an impl and an `extend` for one instance apply to that
  instance only (`TYPE-104` for another, `TYPE-126` through a bound), also mixed with
  a parameter.
- The five built-in heads each through a direct call, the generic ones through a
  bound and a qualified call; an alias of a collection; a native method winning;
  bare `extend List` and `extend Box` keeping their meaning.
- Overlap and duplicates: one impl per head whatever the arguments, an array and a
  `List`, one inherent definition per name for a generic head and for a run-time kind
  (`int` and `long`).
- Every unsupported target, with its exact text and position, and the count
  mistakes (`TYPE-108`).

**Mutation pass:** 74 mutations over `sema/type_infer/impl_patterns.rs`,
`sema/type_infer.rs`, `ast/types.rs` and `interpreter/value.rs`, 70 caught. Three survivors are equivalent mutants (the run-time key text sema gives `Dictionary`, which is only compared with itself, and the undecided answer of an impl guard, which every consumer treats like a satisfied one because an obligation fails only on a definite no); one more showed an arity entry for a built-in head's definition that no reader reaches, and was removed as dead code. Every other mutant was caught, after tests were added for the first survivors.

### `tests/method_tiers.rs`

**What it does:** Integration tests for the tier of a method and the tier check on a call
on a receiver, slice S4a (43 tests). Programs go through `check_source` for stage, codes,
messages and positions and through the interpreter for run-time values.

**Decisions:**
- The call rule: every caller and callee tier pair generated into a program; the message
  naming both tiers and the method; the error placed at the call; one diagnostic per
  offending call; every route to a method (an `extend`, an `impl`, a struct member, a
  trait impl, an associated function, a method of a built-in type, a generic struct, a
  generic impl for one instance, a sibling method), and a built-in method carrying no tier.
- Where a tier comes from: an unrelated attribute never changes it; a method's own `@tier`
  wins over the `impl`, `extend`, struct and tier block around it; a tier block moves only
  the methods that write none; an item's own tier survives a block; the innermost block
  wins; an `async` method that inherits `LOW` is `TIER-003`, one that writes `@tier(high)`
  is fine.
- The cases that first sat here as "not checked yet" (a call through a bound, a default
  method call, a `@tier` on a trait default method) became checks in S4b and moved to
  `tests/trait_tiers.rs`.
- Run time unchanged: `LOW` methods called from `LOW` code run.

**Mutation pass:** 31 mutations over `parsers/parse_attr.rs`, `parsers/parse_decl.rs`,
`prelude.rs`, `sema/type_infer.rs` and `sema/tier_check.rs`, 30 caught. Two were caught only
after three tests were added (a tier block around an `impl`, an `extend` or a struct whose
methods are partly tiered); the survivor is an equivalent mutant (the copy of a trait
shape's tier, `None` for every trait shape until S4b). The mutant that removes the
prelude's `Hasher` tier is caught by `tests/prelude_traits.rs`.

### `tests/trait_tiers.rs`

**What it does:** Integration tests for tiers on traits, slice S4b (47 tests). Programs go
through `check_source` for stage, codes, messages, suggestions and positions and through the
interpreter for run-time values.

**Decisions:**
- Where a trait method's tier comes from: a trait with none is `HIGH`; a `@tier` on a
  signature or a default method is applied (and a default method's body is checked at it);
  the trait's `@tier` is the default; a method's own tier wins; an unrelated attribute
  changes nothing; a tier block moves only the signatures and default methods that write
  none; an item's own tier survives a block; the innermost block wins.
- Calls: every caller and declared tier pair, generated into a program, through a bound
  and through a qualified call; the message and the position; the abstract `Self` in a
  default body; a supertrait bound; a bounded struct's method; a default method on a
  concrete receiver.
- Conformance: every trait and impl tier pair under exact match (equal accepted, higher and
  lower `TIER-016`); the message, the suggestion and the position; one report per method; an
  unrelated attribute is not a written tier; an impl method that writes none takes the
  trait's (its body is checked at that tier and a call to it uses it); a tier written on the
  method, the `impl` (even `@tier(high)`) or a tier block around it is a written tier; an
  override of a default method; an impl for a built-in type and a generic impl; a mismatched
  method is still called at its own tier.
- The prelude traits are `LOW`: every tier can call their methods through a bound; a
  hand-written impl has to be `LOW`, and its body may not call a `HIGH` function (`TIER-005`),
  read a `static` (`TIER-015`) or build a collection (`TIER-009`); derived and native
  methods carry no tier.
- `async` keeps its rule, and the run-time results are unchanged.

**Mutation pass:** 33 mutations over `parsers/parse_decl.rs`, `prelude.rs`, `sema/type_infer.rs`,
`sema/tier_check.rs`, `sema/sema_context.rs` and `errors/tier/mod.rs`, 30 caught. Two were
caught only after tests were added (a default method that writes its own tier inside a tier
block around a trait, and a mismatched impl method that is still called at its own tier). One
condition was redundant and was removed instead of tested (a tier block merges its own
`@tier` into the impl's attributes, so checking the impl's tier field as well added nothing).
Two are equivalent mutants: the fallback tier of a trait method shape (every trait method
shape carries a tier), and the tier on `Eq`, which has no methods.

### `tests/builtin_impls.rs`

**What it does:** Integration tests for impls and `extend` blocks on built-in
types, slice S3d (51 tests), plus a unit test in `interpreter/value.rs` that pins
`Value::impl_key` against `ast::types::builtin_impl_key` for every spelling of every
type. Programs go through `check_source` for stage, codes, messages and positions
and through the interpreter for run-time values.

**Decisions:**
- Every primitive spelling is generated into a program that implements a trait,
  calls it as a method, through a bound and in the qualified form, and takes an
  `extend` block; each prints its own spelling.
- Calls: a method with arguments, a literal in parentheses, `Self`, default methods
  three ways, a generic method on a primitive, a generic trait with its argument,
  aliases of primitives and of tuples, the body of an impl checked with `self` as the
  built-in type, `Self` in an `extend` signature.
- Sema keeps types apart: an impl for `int` does not satisfy `long`, a missing impl
  is `TYPE-104` or `TYPE-126`, types of different kinds each have an impl.
- One kind of value, one impl: `int`/`long` is `TYPE-128` with the exact text, every
  pair inside each of the seven kind groups, the same type twice, two traits on types
  of one kind, tuples of one arity (and of different arity side by side), two array
  types.
- Native methods win over `extend`, an `extend` method beats a trait method and the
  qualified form reaches the trait, two traits with one name are `TYPE-127`, the
  qualified form picks one, supertraits are required of a primitive impl, missing
  and mismatched methods are reported.
- Prelude traits: refused where native (`Eq` for `int`, `Hash` for `string`, `Clone`
  for `bool`, `PartialEq` for a tuple) with the exact message, allowed where not
  (`Hash` for `double` makes a dictionary key), needing their supertraits, ordering
  operators on `bool` and `char`, a hand-written impl used through a bound and in the
  qualified form.
- A method that does not exist on a primitive is `TYPE-104` (it used to panic), a
  function without `self` is not a method, an array parameter keeps its native
  methods, `impl` and `extend` for a collection are `TYPE-129`, errors inside a
  built-in target are reported once, and structs and enums keep working beside all
  of it.
- Four fixtures sit beside it: `ok_builtin_impl_isolated`, `err_builtin_impl_isolated`,
  `ok_builtin_impl_combined` and `err_builtin_impl_combined`. The two tests that
  pinned built-in impls as `TYPE-129` (`tests/traits.rs`, `tests/prelude_traits.rs`)
  now assert the new behaviour.
- Mutation-checked: 39 mutations of the key mapping, the definitions, the
  conversion of a target, the overlap rules, the prelude status, the bound checks,
  the call hooks, the `Self` handling and the interpreter's registration and
  dispatch. The first pass left six alive. Four needed tests (an arity error inside
  a tuple target reported once, `Self` in an `extend` signature, a native method on
  an array parameter, the key unit test), and one of those found a real gap, `self`
  untyped in the body of a built-in impl, which is fixed. One conjunct of the
  primitive check could not matter and was removed, and one change to the qualified
  call turned out to be unneeded and was reverted. Run again on the finished code,
  38 of the 39 are caught. The one that is not changes nothing a program can
  observe: the empty-table fast path in `builtin_def_of`, kept because it spares a
  type-text allocation on every method call of a program without built-in impls.

### `prelude.rs`, `tests/prelude_traits.rs`

**What it does:** `prelude.rs` holds the declarations every program sees
without writing them (`Ordering`, `PartialEq`, `Eq`, `PartialOrd`, `Ord`,
`Clone`, `Hasher`, `Hash`) as Ubel source. `Hasher` and the six traits are `@tier(low)`, so code at any tier can
hash and call their methods through a bound: the prelude is ordinary source and its methods are `HIGH` unless a tier is written.
`parse()` calls `prelude::inject` after parsing the
program. `tests/prelude_traits.rs` (95 tests) covers what each kind of type
satisfies, the methods, hashing, the `Dictionary` key requirement, the
operators on a type parameter, hand-written impls and what calls them
(operators, `contains`, dictionaries, `order_by`, `group_by`, derived
comparison, `Hash` and `Clone` inside other values) and shadowing.

**Decisions:**
- The prelude source is lexed once per process and every token span is
  shifted past `PRELUDE_SPAN_START`, so a prelude node cannot collide with a
  user node in sema's `Span`-keyed tables. The parser never slices the source
  by span, so this needs no other change.
- The reserved range starts at 2^31, computed from a `u32`, because `wasm32`
  has a 32-bit `usize`. The first version used 2^40, which compiled natively
  and failed the wasm playground build (`shift left by 40 would overflow`) when
  it was deployed; a 64-bit build cannot see that, so a unit test checks that
  the range and every prelude token span fit in a `u32`.
- A name the program declares itself is not injected, nor is anything that
  depends on it, so a prelude signature never binds to a user's type. What an
  item depends on is read from its declaration (its supertraits, now written in
  the prelude source: `trait Ord: PartialOrd + Eq`) plus the types its
  signatures mention (`SIGNATURE_DEPS`). Eleven unit tests in `prelude.rs` pin
  the dependencies, their order and the shadowing sets.
- The tests pin the satisfaction table kind by kind, the exact code and
  position of `TYPE-126`, `TYPE-131` and `TYPE-129`, and that a call through a
  bound or directly on a value runs the right native method.
- Mutation-checked: widening floats to `Eq` and `Ord` fails three tests; no
  supertrait expansion fails three; no operator bound check fails four; no
  prelude methods on values fails one; accepting hand-written impls fails two;
  no native prelude methods fails six; a shadowing rule that does not cascade
  fails five; enums that are not `PartialEq` fail one; collections that
  satisfy fail two; ordered tuples fail one. For part 2: floats that are
  `Hash` fail seven tests; enums that are not `Hash` fail three; no key check
  at an annotation fails five, at `Dictionary.new()` one, at the first `set` of
  an inferred dictionary one; an alias not checked at its use fails one and an
  alias checked at its declaration fails one; no same-line dedupe fails four; a
  `hash` that does nothing fails three; hashing that is not order-sensitive
  fails one; `Hash` without its supertraits fails one. For S2b: 69 mutations of
  the dispatch and the impl checks, each on the function itself, 68 are caught.
  The one that is not, the bit rotation in the hash mixer, changes no result a
  program can observe, because hash values are not a contract. The first pass
  left 17 alive and exposed missing tests (tuple, `Unique` and enum payload
  operands, a derived ordering that must move on to the next field, the `x.eq(y)`
  and `x.lt(y)` call forms, `group_by`, dictionary index assignment, inline-list
  `contains`, an impl written after the signature that needs it); each now has
  one.

### `tests/bounds.rs`

**What it does:** Integration tests for trait bounds, slice S1b (45
tests). Source goes through `check_source` for the stage, codes, messages and
positions, and through the interpreter (sema, then both setters, then
`run_program`) where a test needs runtime values.

**Decisions:**
- Pins the exact code, count and position of every bound diagnostic
  (`TYPE-122`, `TYPE-126`, `TYPE-129`, `TYPE-130`, and `TYPE-104` for a bound
  set that lacks the method), that an integer literal is reported as `int`,
  that diagnostics come out in source order, and the runtime contract that a
  call through a bound runs the trait's method even when the type has an
  inherent method of the same name. One test runs without
  `set_trait_call_sites` to document why that table exists.
- Covers each place a bound is enforced: a function call, a call through a
  variable, a list argument, a struct literal, an enum variant, a type
  annotation and a struct field, plus the places a bound must be visible
  (a generic struct's method, an `extend` block, a MID-tier function with an
  arena value).
- Mutation-checked: removing the `TYPE-130` report fails the four tests that
  expect it; removing the obligations at a function call fails the eight
  call-site tests; removing them at struct and enum construction fails three
  tests, and at type annotations three; installing no bounds for an `extend`
  body fails the two extend tests; checking obligations before literal types
  are settled fails six; and removing the span sort fails only the nested-call
  ordering test.

### `tests/statics.rs`

**What it does:** Integration tests for `static` items. Source goes
through `check_source` for the stage and diagnostic codes, and through
the interpreter (sema first, then `set_int_literal_types`, then
`run_program`) where a test needs runtime values.

**Decisions:**
- Pins what fixtures cannot see: the exact code and count per rule
  (`TIER-015`, `NAME-008`, `PARSE-004`, `PARSE-001`), that a malformed
  declaration is reported once, that a tier block around a const or
  static is not an own annotation, and the runtime contract that a
  static is one value shared by every function.
- Mutation-checked: disabling the static branch of `write_lvalue` fails
  exactly `a_static_is_one_value_shared_by_every_function`,
  `assignment_in_main_is_visible_to_a_function_and_back` and
  `a_closure_writes_the_shared_static`.

### `tests/int_literal_typing.rs`

**What it does:** Integration tests for context-driven typing of
unsuffixed integer literals. Source text goes through lex, parse and sema,
and where a test needs runtime values, through the interpreter with
output capture. Three helpers (`type_error_messages`, `recorded_widths`,
`run`) keep each test to one source string and one assertion.

**Decisions:**
- Covers what the `.ubl` fixtures cannot see: exact diagnostic counts and
  messages for `TYPE-120` at every boundary, which literals sema records
  in `int_literal_types`, that a run without that table panics on a
  sized/plain mix instead of guessing, and the two places an open literal
  is settled as `int` (end of body, type-dependent format spec).
- The interpreter is driven the same way `ubel run` drives it: sema
  first, then `set_int_literal_types`, then `run_program`.

### `check.rs`

**What it does:** `check_source(&str) -> CheckReport` runs lex, parse and
sema over a source string and returns every diagnostic as data, together
with the stage it stopped at (`Stage::Lex`, `Parse`, `Sema`, or `Clean`).
It is the one entry point both `ubel` (the command line) and `ubel-lsp`
(the language server) call, so the two report identical diagnostics for
the same text.

**Decisions:**
- Stops at the first stage that reports errors, since later stages have
  nothing sound to work on once an earlier one failed. Sema's own passes
  keep running after each other inside `analyse`, as before.
- Every per-phase error list is drained through the `Diagnosable` trait
  into one flat, phase-ordered `Vec<Diagnostic>`. Nothing here prints or
  renders; callers choose `render_all`, JSON, or LSP diagnostics.
- The AST borrows the `AstArena`, so the arena cannot leave the function.
  A caller that needs to run the program afterwards (`ubel run`) lexes and
  parses a second time. A `check_with_arena` variant is the fix if that
  cost ever matters.

**Tests:** unit tests in `check.rs`, and end to end through
`crates/cli/tests/cli_tests.rs` and `crates/lsp/tests/lsp_stdio.rs`.

### `cursor.rs`, `parsers/parse_type.rs`

**What it does:** `cursor.rs` is the token cursor every parser shares;
`parse_type.rs` parses type expressions, generic arguments and generic
declaration parameters.

**Decisions:**
- `Cursor` splits a `>>` token to close two nested generic argument
  lists (`at_generic_close`, `eat_generic_close`, `expect_generic_close`,
  private `split_at`). The lexer cannot make this call, only the parser
  knows a position is a type position. See `PARSER_RULES.md` §5.9.

**Tests:** `crates/rd_parser/tests/diagnostic_counts.rs`,
`tests/fixtures/ok_nested_generic_close_isolated.ubl`,
`tests/fixtures/ok_nested_generic_close_combined.ubl`,
`tests/fixtures/err_nested_generic_unclosed.ubl`.

### `parser.rs`, `parsers/parse_expr.rs`, `parsers/parse_stmt.rs`

**What it does:** `parser.rs` is the `Parser` struct and its shared
`enter`/`leave`-style state-swap helpers; `parse_expr.rs` is the Pratt
expression parser; `parse_stmt.rs` is statement parsing (`if`/`while`/
`for`/`match`/etc.) at the statement-position half of the grammar
(expression-position `if`/`match` live in `parse_expr.rs` instead, see
`PARSER_RULES.md` §4 for why the grammar has both).

**Decisions:**
- `Parser` gained a `no_struct_lit: bool` field plus paired
  `enter_no_struct_lit`/`clear_no_struct_lit`/`leave_no_struct_lit`
  helpers, mirroring the existing `tier`/`enter_tier`/`leave_tier`
  pattern exactly. See `PARSER_RULES.md` §5.8 for the full disambiguation
  writeup — the short version: a bare identifier condition followed by a
  block whose first statement is a plain assignment was indistinguishable
  from a struct literal from 2 tokens of lookahead, and no amount of
  additional lookahead can fix that in general, so the fix suppresses
  struct-literal parsing entirely for the condition/iterable/scrutinee of
  `if`/`elif`/`while`/`for`/`match` (6 call sites total, statement- and
  expression-position both), clearing it again inside any real bracket
  pair (`(...)`, call args, `[...]`) so a parenthesized struct literal
  still works as the escape hatch.

**Tests:** `tests/fixtures/ok_condition_struct_lit_ambiguity_isolated.ubl`,
`tests/fixtures/ok_condition_struct_lit_ambiguity_combined.ubl`.

## CI and Workflows

- `.github/workflows/ci-check.yml`, "Ubel Stratum, Fast Compile Check":
  runs on every push and pull request to `master`/`main`, covers this
  crate along with `ubel_stratum`.
- `.github/workflows/parser-crate-migrate.yml`: migration workflow for
  this crate's split from the original monolithic parser.

## Fixes and Problems

### `parsers/parse_pattern.rs`

- Both of this file's wildcard-recognizing arms, one for match-arm
  patterns and one for struct-destructure fields, checked for
  `TokenType::Ident(n) if n == "_"`. The lexer tokenizes a bare `_` as
  its own token, `TokenType::Underscore`, specifically so it is never
  `Ident("_")`. That meant neither arm could ever fire for a real `_`
  in source text: writing `_ => ...` in a match produced a parse error
  instead of a wildcard match, even though `PatternKind::Wildcard`
  itself was already fully implemented and correct everywhere it was
  consumed (name resolution, `PatternCoverage::CatchAll` in type
  inference, the interpreter's pattern matcher). Confirmed via a real
  `_ => ...` match arm before fixing anything: the parser's own error
  message listed `'_'` as an expected token right next to the
  `Underscore` token it had actually received. Fixed by checking
  `TokenType::Underscore` directly in both places instead.

### `parser.rs`, `parsers/parse_expr.rs`, `parsers/parse_stmt.rs`

- A bare identifier used as an `if`/`while`/`for`/`match`
  condition/iterable/scrutinee, immediately followed by a block whose
  first statement was a plain assignment, misparsed as a struct literal
  — `if x == y { hit_count = hit_count + 1 }` read as `y { hit_count =
  hit_count + 1 }`, one field named `hit_count`. Found via exploratory
  testing (a nested-loop diagonal grid scan), root-caused directly
  against `is_struct_open`'s 2-token lookahead rather than guessed at:
  `{ Ident Equal` is genuinely ambiguous between a struct literal's first
  field and a block's first statement, not a lookahead-depth problem.
  Fixed the way Rust resolves the same ambiguity — suppress struct-literal
  parsing for the condition/iterable/scrutinee itself (a new
  `no_struct_lit` restriction on `Parser`), not by adding more lookahead.
  A first attempt broke the parenthesized escape hatch
  (`if (Foo { x = 1 }).ready { ... }`) by leaving the restriction on for
  everything nested inside the condition, parens included — caught by
  actually running the fixture that exercises it, which failed with an
  unclosed-`(` error, not by inspection. Fixed by clearing the
  restriction again inside any bracket pair the parser itself must
  match a close for (`(...)`, call args, `[...]`), where the ambiguity
  cannot occur regardless. See `PARSER_RULES.md` §5.8 for the full
  writeup.

### `cursor.rs`, `parsers/parse_type.rs`, `parsers/parse_expr.rs`

- `List<List<int>>` did not parse. `>>` is one `RightShift` token and
  every generic closer expected a plain `Greater`. Fixed in the cursor by
  splitting the token, not in the lexer, which cannot know it is in a
  type position. The half-consumed mark is cleared by `restore` so a
  speculative parse cannot leak it into a retry.
- A lambda return type annotation (`fn(x: int) string { ... }`) parsed
  the type name as the whole body and reported two unrelated name errors.
  `parse_lambda` now looks ahead for `Type {`, reports one `PARSE-004` on
  the annotation, and parses the block normally. A `Point { x = a }`
  struct literal body is not mistaken for an annotation. The first
  version only recognized identifier-led types and missed `List<int>`,
  because collection types are dedicated keyword tokens, not identifiers;
  found by running the probe rather than by reading the code.
