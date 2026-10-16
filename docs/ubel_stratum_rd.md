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

### `prelude.rs`, `tests/prelude_traits.rs`

**What it does:** `prelude.rs` holds the declarations every program sees
without writing them (`Ordering`, `PartialEq`, `Eq`, `PartialOrd`, `Ord`,
`Clone`, `Hasher`, `Hash`) as Ubel source. `parse()` calls `prelude::inject` after parsing the
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
