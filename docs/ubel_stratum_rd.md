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

**Tests:** see `tests/fixtures/ok_wildcard_and_discard_isolated.ubl` and
`tests/fixtures/ok_callback_registry_combined.ubl`.

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
