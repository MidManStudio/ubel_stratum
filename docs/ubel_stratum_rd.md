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

**Tests:** see `tests/fixtures/ok_wildcard_and_discard_isolated.ubl` and
`tests/fixtures/ok_callback_registry_combined.ubl`; for `static`, the
five `ok_static_*` and seven `err_*static*`/`err_const_tier_*` fixtures
and `tests/statics.rs` below.

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
